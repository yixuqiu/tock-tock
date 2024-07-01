// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! Library of randomness structures, including a system call driver for
//! userspace applications to request randomness, entropy conversion, entropy
//! to randomness conversion, and synchronous random number generation.
//!
//!
//! The RNG accepts a user-defined callback and buffer to hold received
//! randomness. A single command starts the RNG, the callback is called when the
//! requested amount of randomness is received, or the buffer is filled.
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! let rng = static_init!(
//!     capsules::rng::RngDriver<'static, sam4l::trng::Trng>,
//!     capsules::rng::RngDriver::new(&sam4l::trng::TRNG, board_kernel.create_grant(&grant_cap)),
//! );
//! sam4l::trng::TRNG.set_client(rng);
//! ```

use core::cell::Cell;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil::entropy;
use kernel::hil::entropy::{Entropy32, Entropy8};
use kernel::hil::rng;
use kernel::hil::rng::{Client, Continue, Random, Rng};
use kernel::processbuffer::WriteableProcessBuffer;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::OptionalCell;
use kernel::{ErrorCode, ProcessId};

/// Syscall driver number.
use crate::driver;
pub const DRIVER_NUM: usize = driver::NUM::Rng as usize;

/// Ids for read-write allow buffers
mod rw_allow {
    pub const BUFFER: usize = 0;
    /// The number of allow buffers the kernel stores for this grant
    pub const COUNT: u8 = 1;
}

#[derive(Default)]
pub struct App {
    remaining: usize,
    idx: usize,
}

pub struct RngDriver<'a, R: Rng<'a>> {
    rng: &'a R,
    apps: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<{ rw_allow::COUNT }>>,
    getting_randomness: Cell<bool>,
}

impl<'a, R: Rng<'a>> RngDriver<'a, R> {
    pub fn new(
        rng: &'a R,
        grant: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<{ rw_allow::COUNT }>>,
    ) -> Self {
        Self {
            rng: rng,
            apps: grant,
            getting_randomness: Cell::new(false),
        }
    }
}

impl<'a, R: Rng<'a>> rng::Client for RngDriver<'a, R> {
    fn randomness_available(
        &self,
        randomness: &mut dyn Iterator<Item = u32>,
        _error: Result<(), ErrorCode>,
    ) -> rng::Continue {
        let mut done = true;
        for cntr in self.apps.iter() {
            cntr.enter(|app, kernel_data| {
                // Check if this app needs random values.
                if app.remaining > 0 {
                    // Provide the current application values to the closure
                    let (oldidx, oldremaining) = (app.idx, app.remaining);

                    let (newidx, newremaining) = kernel_data
                        .get_readwrite_processbuffer(rw_allow::BUFFER)
                        .and_then(|buffer| {
                            buffer.mut_enter(|buffer| {
                                let mut idx = oldidx;
                                let mut remaining = oldremaining;

                                // Check that the app is not asking for more than can
                                // fit in the provided buffer
                                if buffer.len() < idx {
                                    // The buffer does not fit at all
                                    // anymore (the app must've swapped
                                    // buffers), end the operation
                                    return (0, 0);
                                } else if buffer.len() < idx + remaining {
                                    remaining = buffer.len() - idx;
                                }

                                // Add all available and requested randomness to the app buffer.

                                // 1. Slice buffer to start from current idx
                                let buf = &buffer[idx..(idx + remaining)];
                                // 2. Take at most as many random samples as needed to fill the buffer
                                //    (if app.remaining is not word-sized, take an extra one).
                                let remaining_ints = if remaining % 4 == 0 {
                                    remaining / 4
                                } else {
                                    remaining / 4 + 1
                                };

                                // 3. Zip over the randomness iterator and chunks
                                //    of up to 4 bytes from the buffer.
                                for (inp, outs) in
                                    randomness.take(remaining_ints).zip(buf.chunks(4))
                                {
                                    // 4. For each word of randomness input, update
                                    //    the remaining and idx and add to buffer.
                                    let inbytes = u32::to_le_bytes(inp);
                                    outs.iter().zip(inbytes.iter()).for_each(|(out, inb)| {
                                        out.set(*inb);
                                        remaining -= 1;
                                        idx += 1;
                                    });
                                }

                                (idx, remaining)
                            })
                        })
                        .unwrap_or(
                            // If the process is no longer alive
                            // (or this is a default AppSlice),
                            // set the idx and remaining values of
                            // this app to (0, 0)
                            (0, 0),
                        );

                    // Store the updated values in the application
                    app.idx = newidx;
                    app.remaining = newremaining;

                    if app.remaining > 0 {
                        done = false;
                    } else {
                        kernel_data.schedule_upcall(0, (0, newidx, 0)).ok();
                    }
                }
            });

            // Check if done switched to false. If it did, then that app
            // didn't get enough random, so there's no way there is more for
            // other apps.
            if !done {
                break;
            }
        }

        if done {
            self.getting_randomness.set(false);
            rng::Continue::Done
        } else {
            rng::Continue::More
        }
    }
}

impl<'a, R: Rng<'a>> SyscallDriver for RngDriver<'a, R> {
    fn command(
        &self,
        command_num: usize,
        data: usize,
        _: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        match command_num {
            // Driver existence check
            0 => CommandReturn::success(),

            // Ask for a given number of random bytes
            1 => {
                let mut needs_get = false;
                let result = self
                    .apps
                    .enter(processid, |app, _| {
                        app.remaining = data;
                        app.idx = 0;

                        // Assume that the process has a callback & slice
                        // set. It might die or revoke them before the
                        // result arrives anyways
                        if !self.getting_randomness.get() {
                            self.getting_randomness.set(true);
                            needs_get = true;
                        }

                        CommandReturn::success()
                    })
                    .unwrap_or_else(|err| CommandReturn::failure(err.into()));
                if needs_get {
                    let _ = self.rng.get();
                }
                result
            }
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.apps.enter(processid, |_, _| {})
    }
}

pub struct Entropy32ToRandom<'a, E: Entropy32<'a>> {
    egen: &'a E,
    client: OptionalCell<&'a dyn rng::Client>,
}

impl<'a, E: Entropy32<'a>> Entropy32ToRandom<'a, E> {
    pub fn new(egen: &'a E) -> Self {
        Self {
            egen: egen,
            client: OptionalCell::empty(),
        }
    }
}

impl<'a, E: Entropy32<'a>> Rng<'a> for Entropy32ToRandom<'a, E> {
    fn get(&self) -> Result<(), ErrorCode> {
        self.egen.get()
    }

    fn cancel(&self) -> Result<(), ErrorCode> {
        self.egen.cancel()
    }

    fn set_client(&'a self, client: &'a dyn rng::Client) {
        self.egen.set_client(self);
        self.client.set(client);
    }
}

impl<'a, E: Entropy32<'a>> entropy::Client32 for Entropy32ToRandom<'a, E> {
    fn entropy_available(
        &self,
        entropy: &mut dyn Iterator<Item = u32>,
        error: Result<(), ErrorCode>,
    ) -> entropy::Continue {
        self.client.map_or(entropy::Continue::Done, |client| {
            if error != Ok(()) {
                match client.randomness_available(&mut Entropy32ToRandomIter(entropy), error) {
                    rng::Continue::More => entropy::Continue::More,
                    rng::Continue::Done => entropy::Continue::Done,
                }
            } else {
                match client.randomness_available(&mut Entropy32ToRandomIter(entropy), Ok(())) {
                    rng::Continue::More => entropy::Continue::More,
                    rng::Continue::Done => entropy::Continue::Done,
                }
            }
        })
    }
}

struct Entropy32ToRandomIter<'a>(&'a mut dyn Iterator<Item = u32>);

impl Iterator for Entropy32ToRandomIter<'_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        self.0.next()
    }
}

pub struct Entropy8To32<'a, E: Entropy8<'a>> {
    egen: &'a E,
    client: OptionalCell<&'a dyn entropy::Client32>,
    count: Cell<usize>,
    bytes: Cell<u32>,
}

impl<'a, E: Entropy8<'a>> Entropy8To32<'a, E> {
    pub fn new(egen: &'a E) -> Self {
        Self {
            egen: egen,
            client: OptionalCell::empty(),
            count: Cell::new(0),
            bytes: Cell::new(0),
        }
    }
}

impl<'a, E: Entropy8<'a>> Entropy32<'a> for Entropy8To32<'a, E> {
    fn get(&self) -> Result<(), ErrorCode> {
        self.egen.get()
    }

    /// Cancel acquisition of random numbers.
    ///
    /// There are two valid return values:
    ///   - Ok(()): an outstanding request from `get` has been cancelled,
    ///     or there was no outstanding request. No `randomness_available`
    ///     callback will be issued.
    ///   - FAIL: There will be a randomness_available callback, which
    ///     may or may not return an error code.
    fn cancel(&self) -> Result<(), ErrorCode> {
        self.egen.cancel()
    }

    fn set_client(&'a self, client: &'a dyn entropy::Client32) {
        self.egen.set_client(self);
        self.client.set(client);
    }
}

impl<'a, E: Entropy8<'a>> entropy::Client8 for Entropy8To32<'a, E> {
    fn entropy_available(
        &self,
        entropy: &mut dyn Iterator<Item = u8>,
        error: Result<(), ErrorCode>,
    ) -> entropy::Continue {
        self.client.map_or(entropy::Continue::Done, |client| {
            if error != Ok(()) {
                client.entropy_available(&mut Entropy8To32Iter(self), error)
            } else {
                let mut count = self.count.get();
                // Read in one byte at a time until we have 4;
                // return More if we need more, else return the value
                // of the upper randomness_available, as if it needs more
                // we'll need more from the underlying Rng8.
                while count < 4 {
                    let byte = entropy.next();
                    match byte {
                        None => {
                            return entropy::Continue::More;
                        }
                        Some(val) => {
                            let current = self.bytes.get();
                            let bits = val as u32;
                            let result = current | (bits << (8 * count));
                            count += 1;
                            self.count.set(count);
                            self.bytes.set(result)
                        }
                    }
                }
                let rval = client.entropy_available(&mut Entropy8To32Iter(self), Ok(()));
                self.bytes.set(0);
                rval
            }
        })
    }
}

struct Entropy8To32Iter<'a, 'b: 'a, E: Entropy8<'b>>(&'a Entropy8To32<'b, E>);

impl<'a, 'b: 'a, E: Entropy8<'b>> Iterator for Entropy8To32Iter<'a, 'b, E> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        let count = self.0.count.get();
        if count == 4 {
            self.0.count.set(0);
            Some(self.0.bytes.get())
        } else {
            None
        }
    }
}

pub struct Entropy32To8<'a, E: Entropy32<'a>> {
    egen: &'a E,
    client: OptionalCell<&'a dyn entropy::Client8>,
    entropy: Cell<u32>,
    bytes_consumed: Cell<usize>,
}

impl<'a, E: Entropy32<'a>> Entropy32To8<'a, E> {
    pub fn new(egen: &'a E) -> Self {
        Self {
            egen: egen,
            client: OptionalCell::empty(),
            entropy: Cell::new(0),
            bytes_consumed: Cell::new(0),
        }
    }
}

impl<'a, E: Entropy32<'a>> Entropy8<'a> for Entropy32To8<'a, E> {
    fn get(&self) -> Result<(), ErrorCode> {
        self.egen.get()
    }

    /// Cancel acquisition of random numbers.
    ///
    /// There are two valid return values:
    ///   - Ok(()): an outstanding request from `get` has been cancelled,
    ///     or there was no outstanding request. No `randomness_available`
    ///     callback will be issued.
    ///   - FAIL: There will be a randomness_available callback, which
    ///     may or may not return an error code.
    fn cancel(&self) -> Result<(), ErrorCode> {
        self.egen.cancel()
    }

    fn set_client(&'a self, client: &'a dyn entropy::Client8) {
        self.egen.set_client(self);
        self.client.set(client);
    }
}

impl<'a, E: Entropy32<'a>> entropy::Client32 for Entropy32To8<'a, E> {
    fn entropy_available(
        &self,
        entropy: &mut dyn Iterator<Item = u32>,
        error: Result<(), ErrorCode>,
    ) -> entropy::Continue {
        self.client.map_or(entropy::Continue::Done, |client| {
            if error != Ok(()) {
                client.entropy_available(&mut Entropy32To8Iter::<E>(self), error)
            } else {
                let r = entropy.next();
                match r {
                    None => return entropy::Continue::More,
                    Some(val) => {
                        self.entropy.set(val);
                        self.bytes_consumed.set(0);
                    }
                }
                client.entropy_available(&mut Entropy32To8Iter(self), Ok(()))
            }
        })
    }
}

struct Entropy32To8Iter<'a, 'b: 'a, E: Entropy32<'b>>(&'a Entropy32To8<'b, E>);

impl<'a, 'b: 'a, E: Entropy32<'b>> Iterator for Entropy32To8Iter<'a, 'b, E> {
    type Item = u8;

    fn next(&mut self) -> Option<u8> {
        let bytes_consumed = self.0.bytes_consumed.get();
        if bytes_consumed < 4 {
            // Pull out a byte and right shift the u32 so its
            // least significant byte is fresh randomness.
            let entropy = self.0.entropy.get();
            let byte = (entropy & 0xff) as u8;
            self.0.entropy.set(entropy >> 8);
            self.0.bytes_consumed.set(bytes_consumed + 1);
            Some(byte)
        } else {
            None
        }
    }
}

pub struct SynchronousRandom<'a, R: Rng<'a>> {
    rgen: &'a R,
    seed: Cell<u32>,
}

#[allow(dead_code)]
impl<'a, R: Rng<'a>> SynchronousRandom<'a, R> {
    fn new(rgen: &'a R) -> Self {
        Self {
            rgen: rgen,
            seed: Cell::new(0),
        }
    }
}

impl<'a, R: Rng<'a>> Random<'a> for SynchronousRandom<'a, R> {
    fn initialize(&'a self) {
        self.rgen.set_client(self);
        let _ = self.rgen.get();
    }

    fn reseed(&self, seed: u32) {
        self.seed.set(seed);
    }

    // This implementation uses a linear congruential generator due to
    // its efficiency. The parameters for the generator are those
    // recommended in Numerical Recipes by Press, Teukolsky,
    // Vetterling, and Flannery.

    fn random(&self) -> u32 {
        const LCG_MULTIPLIER: u32 = 1_644_525;
        const LCG_INCREMENT: u32 = 1_013_904_223;
        let val = self.seed.get();
        let val = val.wrapping_mul(LCG_MULTIPLIER);
        let val = val.wrapping_add(LCG_INCREMENT);
        self.seed.set(val);
        val
    }
}

impl<'a, R: Rng<'a>> Client for SynchronousRandom<'a, R> {
    fn randomness_available(
        &self,
        randomness: &mut dyn Iterator<Item = u32>,
        _error: Result<(), ErrorCode>,
    ) -> Continue {
        match randomness.next() {
            None => Continue::More,
            Some(val) => {
                self.seed.set(val);
                Continue::Done
            }
        }
    }
}
