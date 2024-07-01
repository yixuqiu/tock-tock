// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! Virtualize a PWM interface.
//!
//! `MuxPwm` provides shared access to a single PWM interface for multiple
//! users. `PwmPinUser` provides access to a specific PWM pin.
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! let mux_pwm = static_init!(
//!     capsules_core::virtual_pwm::MuxPwm<'static, nrf52::pwm::Pwm>,
//!     capsules_core::virtual_pwm::MuxPwm::new(&base_peripherals.pwm0)
//! );
//! let virtual_pwm_buzzer = static_init!(
//!     capsules_core::virtual_pwm::PwmPinUser<'static, nrf52::pwm::Pwm>,
//!     capsules_core::virtual_pwm::PwmPinUser::new(mux_pwm, nrf5x::pinmux::Pinmux::new(31))
//! );
//! virtual_pwm_buzzer.add_to_mux();
//! ```

use kernel::collections::list::{List, ListLink, ListNode};
use kernel::hil;
use kernel::utilities::cells::OptionalCell;
use kernel::ErrorCode;

pub struct MuxPwm<'a, P: hil::pwm::Pwm> {
    pwm: &'a P,
    devices: List<'a, PwmPinUser<'a, P>>,
    inflight: OptionalCell<&'a PwmPinUser<'a, P>>,
}

impl<'a, P: hil::pwm::Pwm> MuxPwm<'a, P> {
    pub const fn new(pwm: &'a P) -> MuxPwm<'a, P> {
        MuxPwm {
            pwm: pwm,
            devices: List::new(),
            inflight: OptionalCell::empty(),
        }
    }

    /// If we are not currently doing anything, scan the list of devices for
    /// one with an outstanding operation and run that.
    fn do_next_op(&self) {
        if self.inflight.is_none() {
            let mnode = self.devices.iter().find(|node| node.operation.is_some());
            mnode.map(|node| {
                let started = node.operation.take().map_or(false, |operation| {
                    match operation {
                        Operation::Simple {
                            frequency_hz,
                            duty_cycle,
                        } => {
                            let _ = self.pwm.start(&node.pin, frequency_hz, duty_cycle);
                            true
                        }
                        Operation::Stop => {
                            // Can't stop if nothing is running
                            false
                        }
                    }
                });
                if started {
                    self.inflight.set(node);
                } else {
                    // Keep looking for something to do.
                    self.do_next_op();
                }
            });
        } else {
            // We are running so we do whatever the inflight user wants, if
            // there is some command there.
            self.inflight.map(|node| {
                node.operation.take().map(|operation| {
                    match operation {
                        Operation::Simple {
                            frequency_hz,
                            duty_cycle,
                        } => {
                            // Changed some parameter.
                            let _ = self.pwm.start(&node.pin, frequency_hz, duty_cycle);
                        }
                        Operation::Stop => {
                            // Ok we got a stop.
                            let _ = self.pwm.stop(&node.pin);
                            self.inflight.clear();
                        }
                    }
                    // Recurse in case there is more to do.
                    self.do_next_op();
                });
            });
        }
    }
}

#[derive(Copy, Clone, PartialEq)]
enum Operation {
    Simple {
        frequency_hz: usize,
        duty_cycle: usize,
    },
    Stop,
}

pub struct PwmPinUser<'a, P: hil::pwm::Pwm> {
    mux: &'a MuxPwm<'a, P>,
    pin: P::Pin,
    operation: OptionalCell<Operation>,
    next: ListLink<'a, PwmPinUser<'a, P>>,
}

impl<'a, P: hil::pwm::Pwm> PwmPinUser<'a, P> {
    pub const fn new(mux: &'a MuxPwm<'a, P>, pin: P::Pin) -> PwmPinUser<'a, P> {
        PwmPinUser {
            mux: mux,
            pin: pin,
            operation: OptionalCell::empty(),
            next: ListLink::empty(),
        }
    }

    pub fn add_to_mux(&'a self) {
        self.mux.devices.push_head(self);
    }
}

impl<'a, P: hil::pwm::Pwm> ListNode<'a, PwmPinUser<'a, P>> for PwmPinUser<'a, P> {
    fn next(&'a self) -> &'a ListLink<'a, PwmPinUser<'a, P>> {
        &self.next
    }
}

impl<P: hil::pwm::Pwm> hil::pwm::PwmPin for PwmPinUser<'_, P> {
    fn start(&self, frequency_hz: usize, duty_cycle: usize) -> Result<(), ErrorCode> {
        self.operation.set(Operation::Simple {
            frequency_hz,
            duty_cycle,
        });
        self.mux.do_next_op();
        Ok(())
    }

    fn stop(&self) -> Result<(), ErrorCode> {
        self.operation.set(Operation::Stop);
        self.mux.do_next_op();
        Ok(())
    }

    fn get_maximum_frequency_hz(&self) -> usize {
        self.mux.pwm.get_maximum_frequency_hz()
    }

    fn get_maximum_duty_cycle(&self) -> usize {
        self.mux.pwm.get_maximum_duty_cycle()
    }
}
