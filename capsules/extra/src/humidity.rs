// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! Provides userspace with access to humidity sensors.
//!
//! Userspace Interface
//! -------------------
//!
//! ### `subscribe` System Call
//!
//! The `subscribe` system call supports the single `subscribe_number` zero,
//! which is used to provide a callback that will return back the result of
//! a humidity reading.
//! The `subscribe`call return codes indicate the following:
//!
//! * `Ok(())`: the callback been successfully been configured.
//! * `ENOSUPPORT`: Invalid allow_num.
//! * `NOMEM`: No sufficient memory available.
//! * `INVAL`: Invalid address of the buffer or other error.
//!
//!
//! ### `command` System Call
//!
//! The `command` system call support one argument `cmd` which is used to specify the specific
//! operation, currently the following cmd's are supported:
//!
//! * `0`: check whether the driver exists
//! * `1`: read humidity
//!
//!
//! The possible return from the 'command' system call indicates the following:
//!
//! * `Ok(())`:    The operation has been successful.
//! * `NOSUPPORT`: Invalid `cmd`.
//! * `NOMEM`:     Insufficient memory available.
//! * `INVAL`:     Invalid address of the buffer or other error.
//!
//! Usage
//! -----
//!
//! You need a device that provides the `hil::sensors::HumidityDriver` trait.
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! let humidity = static_init!(
//!        capsules::humidity::HumiditySensor<'static>,
//!        capsules::humidity::HumiditySensor::new(si7021,
//!                                                board_kernel.create_grant(&grant_cap)));
//! kernel::hil::sensors::HumidityDriver::set_client(si7021, humidity);
//! ```

use core::cell::Cell;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::{ErrorCode, ProcessId};

/// Syscall driver number.
use capsules_core::driver;
pub const DRIVER_NUM: usize = driver::NUM::Humidity as usize;

#[derive(Clone, Copy, PartialEq)]
pub enum HumidityCommand {
    Exists,
    ReadHumidity,
}

#[derive(Default)]
pub struct App {
    subscribed: bool,
}

pub struct HumiditySensor<'a, H: hil::sensors::HumidityDriver<'a>> {
    driver: &'a H,
    apps: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    busy: Cell<bool>,
}

impl<'a, H: hil::sensors::HumidityDriver<'a>> HumiditySensor<'a, H> {
    pub fn new(
        driver: &'a H,
        grant: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    ) -> HumiditySensor<'a, H> {
        HumiditySensor {
            driver: driver,
            apps: grant,
            busy: Cell::new(false),
        }
    }

    fn enqueue_command(
        &self,
        command: HumidityCommand,
        arg1: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        self.apps
            .enter(processid, |app, _| {
                app.subscribed = true;

                if !self.busy.get() {
                    self.busy.set(true);
                    self.call_driver(command, arg1)
                } else {
                    CommandReturn::success()
                }
            })
            .unwrap_or_else(|err| CommandReturn::failure(err.into()))
    }

    fn call_driver(&self, command: HumidityCommand, _: usize) -> CommandReturn {
        match command {
            HumidityCommand::ReadHumidity => self.driver.read_humidity().into(),
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }
}

impl<'a, H: hil::sensors::HumidityDriver<'a>> hil::sensors::HumidityClient
    for HumiditySensor<'a, H>
{
    fn callback(&self, humidity_val: usize) {
        self.busy.set(false);

        for cntr in self.apps.iter() {
            cntr.enter(|app, upcalls| {
                if app.subscribed {
                    app.subscribed = false;
                    upcalls.schedule_upcall(0, (humidity_val, 0, 0)).ok();
                }
            });
        }
    }
}

impl<'a, H: hil::sensors::HumidityDriver<'a>> SyscallDriver for HumiditySensor<'a, H> {
    fn command(
        &self,
        command_num: usize,
        arg1: usize,
        _: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        match command_num {
            // driver existence check
            0 => CommandReturn::success(),

            // single humidity measurement
            1 => self.enqueue_command(HumidityCommand::ReadHumidity, arg1, processid),

            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.apps.enter(processid, |_, _| {})
    }
}
