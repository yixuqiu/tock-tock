// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the LTC294X line of coulomb counters.
//!
//! - <http://www.linear.com/product/LTC2941>
//! - <http://www.linear.com/product/LTC2942>
//! - <http://www.linear.com/product/LTC2943>
//!
//! > The LTC2941 measures battery charge state in battery-supplied handheld PC
//! > and portable product applications. Its operating range is perfectly suited
//! > for single-cell Li-Ion batteries. A precision coulomb counter integrates
//! > current through a sense resistor between the battery’s positive terminal
//! > and the load or charger. The measured charge is stored in internal
//! > registers. An SMBus/I2C interface accesses and configures the device.
//!
//! Structure
//! ---------
//!
//! This file implements the LTC294X driver in two objects. First is the
//! `LTC294X` struct. This implements all of the actual logic for the
//! chip. The second is the `LTC294XDriver` struct. This implements the
//! userland facing syscall interface. These are split to allow the kernel
//! to potentially interface with the LTC294X chip rather than only provide
//! it to userspace.
//!
//! Usage
//! -----
//!
//! Here is a sample usage of this capsule in a board's main.rs file:
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! let buffer = static_init!([u8; capsules::ltc294x::BUF_LEN], [0; capsules::ltc294x::BUF_LEN]);
//! let ltc294x_i2c = static_init!(
//!     capsules::virtual_i2c::I2CDevice,
//!     capsules::virtual_i2c::I2CDevice::new(i2c_mux, 0x64));
//! let ltc294x = static_init!(
//!     capsules::ltc294x::LTC294X<'static>,
//!     capsules::ltc294x::LTC294X::new(ltc294x_i2c, None, buffer));
//! ltc294x_i2c.set_client(ltc294x);
//!
//! // Optionally create the object that provides an interface for the coulomb
//! // counter for applications.
//! let ltc294x_driver = static_init!(
//!     capsules::ltc294x::LTC294XDriver<'static>,
//!     capsules::ltc294x::LTC294XDriver::new(ltc294x));
//! ltc294x.set_client(ltc294x_driver);
//! ```

use core::cell::Cell;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil::gpio;
use kernel::hil::i2c;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::{ErrorCode, ProcessId};

/// Syscall driver number.
use capsules_core::driver;
pub const DRIVER_NUM: usize = driver::NUM::Ltc294x as usize;

pub const BUF_LEN: usize = 20;

#[allow(dead_code)]
enum Registers {
    Status = 0x00,
    Control = 0x01,
    AccumulatedChargeMSB = 0x02,
    AccumulatedChargeLSB = 0x03,
    ChargeThresholdHighMSB = 0x04,
    ChargeThresholdHighLSB = 0x05,
    ChargeThresholdLowMSB = 0x06,
    ChargeThresholdLowLSB = 0x07,
    VoltageMSB = 0x08,
    VoltageLSB = 0x09,
    CurrentMSB = 0x0E,
    CurrentLSB = 0x0F,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,

    /// Simple read states
    ReadStatus,
    ReadCharge,
    ReadVoltage,
    ReadCurrent,
    ReadShutdown,

    Done,
}

/// Which version of the chip we are actually using.
#[derive(Clone, Copy)]
pub enum ChipModel {
    LTC2941 = 1,
    LTC2942 = 2,
    LTC2943 = 3,
}

/// Settings for which interrupt we want.
pub enum InterruptPinConf {
    Disabled = 0x00,
    ChargeCompleteMode = 0x01,
    AlertMode = 0x02,
}

/// Threshold options for battery alerts.
pub enum VBatAlert {
    Off = 0x00,
    Threshold2V8 = 0x01,
    Threshold2V9 = 0x02,
    Threshold3V0 = 0x03,
}

#[derive(Default)]
pub struct App {}

/// Supported events for the LTC294X.
pub trait LTC294XClient {
    fn interrupt(&self);
    fn status(
        &self,
        undervolt_lockout: bool,
        vbat_alert: bool,
        charge_alert_low: bool,
        charge_alert_high: bool,
        accumulated_charge_overflow: bool,
    );
    fn charge(&self, charge: u16);
    fn voltage(&self, voltage: u16);
    fn current(&self, current: u16);
    fn done(&self);
}

/// Implementation of a driver for the LTC294X coulomb counters.
pub struct LTC294X<'a, I: i2c::I2CDevice> {
    i2c: &'a I,
    interrupt_pin: Option<&'a dyn gpio::InterruptPin<'a>>,
    model: Cell<ChipModel>,
    state: Cell<State>,
    buffer: TakeCell<'static, [u8]>,
    client: OptionalCell<&'static dyn LTC294XClient>,
}

impl<'a, I: i2c::I2CDevice> LTC294X<'a, I> {
    pub fn new(
        i2c: &'a I,
        interrupt_pin: Option<&'a dyn gpio::InterruptPin<'a>>,
        buffer: &'static mut [u8],
    ) -> LTC294X<'a, I> {
        LTC294X {
            i2c: i2c,
            interrupt_pin: interrupt_pin,
            model: Cell::new(ChipModel::LTC2941),
            state: Cell::new(State::Idle),
            buffer: TakeCell::new(buffer),
            client: OptionalCell::empty(),
        }
    }

    pub fn set_client<C: LTC294XClient>(&self, client: &'static C) {
        self.client.set(client);

        self.interrupt_pin.map(|interrupt_pin| {
            interrupt_pin.make_input();
            interrupt_pin.enable_interrupts(gpio::InterruptEdge::FallingEdge);
        });
    }

    pub fn read_status(&self) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            // Address pointer automatically resets to the status register.
            // TODO verify errors
            let _ = self.i2c.read(buffer, 1);
            self.state.set(State::ReadStatus);

            Ok(())
        })
    }

    fn configure(
        &self,
        int_pin_conf: InterruptPinConf,
        prescaler: u8,
        vbat_alert: VBatAlert,
    ) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            buffer[0] = Registers::Control as u8;
            buffer[1] = ((int_pin_conf as u8) << 1) | (prescaler << 3) | ((vbat_alert as u8) << 6);

            // TODO verify errors
            let _ = self.i2c.write(buffer, 2);
            self.state.set(State::Done);

            Ok(())
        })
    }

    /// Set the accumulated charge to 0
    fn reset_charge(&self) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            buffer[0] = Registers::AccumulatedChargeMSB as u8;
            buffer[1] = 0;
            buffer[2] = 0;

            // TODO verify errors
            let _ = self.i2c.write(buffer, 3);
            self.state.set(State::Done);

            Ok(())
        })
    }

    fn set_high_threshold(&self, threshold: u16) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            buffer[0] = Registers::ChargeThresholdHighMSB as u8;
            buffer[1] = ((threshold & 0xFF00) >> 8) as u8;
            buffer[2] = (threshold & 0xFF) as u8;

            // TODO verify errors
            let _ = self.i2c.write(buffer, 3);
            self.state.set(State::Done);

            Ok(())
        })
    }

    fn set_low_threshold(&self, threshold: u16) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            buffer[0] = Registers::ChargeThresholdLowMSB as u8;
            buffer[1] = ((threshold & 0xFF00) >> 8) as u8;
            buffer[2] = (threshold & 0xFF) as u8;

            // TODO verify errors
            let _ = self.i2c.write(buffer, 3);
            self.state.set(State::Done);

            Ok(())
        })
    }

    /// Get the cumulative charge as measured by the LTC2941.
    fn get_charge(&self) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            // Read all of the first four registers rather than wasting
            // time writing an address.
            // TODO verify errors
            let _ = self.i2c.read(buffer, 4);
            self.state.set(State::ReadCharge);

            Ok(())
        })
    }

    /// Get the voltage at sense+
    fn get_voltage(&self) -> Result<(), ErrorCode> {
        // Not supported on all versions
        match self.model.get() {
            ChipModel::LTC2942 | ChipModel::LTC2943 => {
                self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
                    self.i2c.enable();

                    // TODO verify errors
                    let _ = self.i2c.read(buffer, 10);
                    self.state.set(State::ReadVoltage);

                    Ok(())
                })
            }
            _ => Err(ErrorCode::NOSUPPORT),
        }
    }

    /// Get the current sensed by the resistor
    fn get_current(&self) -> Result<(), ErrorCode> {
        // Not supported on all versions
        match self.model.get() {
            ChipModel::LTC2943 => self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
                self.i2c.enable();

                // TODO verify errors
                let _ = self.i2c.read(buffer, 16);
                self.state.set(State::ReadCurrent);

                Ok(())
            }),
            _ => Err(ErrorCode::NOSUPPORT),
        }
    }

    /// Put the LTC294X in a low power state.
    fn shutdown(&self) -> Result<(), ErrorCode> {
        self.buffer.take().map_or(Err(ErrorCode::NOMEM), |buffer| {
            self.i2c.enable();

            // Read both the status and control register rather than
            // writing an address.
            // TODO verify errors
            let _ = self.i2c.read(buffer, 2);
            self.state.set(State::ReadShutdown);

            Ok(())
        })
    }

    /// Set the LTC294X model actually on the board.
    fn set_model(&self, model_num: usize) -> Result<(), ErrorCode> {
        match model_num {
            1 => {
                self.model.set(ChipModel::LTC2941);
                Ok(())
            }
            2 => {
                self.model.set(ChipModel::LTC2942);
                Ok(())
            }
            3 => {
                self.model.set(ChipModel::LTC2943);
                Ok(())
            }
            _ => Err(ErrorCode::NODEVICE),
        }
    }
}

impl<I: i2c::I2CDevice> i2c::I2CClient for LTC294X<'_, I> {
    fn command_complete(&self, buffer: &'static mut [u8], _status: Result<(), i2c::Error>) {
        match self.state.get() {
            State::ReadStatus => {
                let status = buffer[0];
                let uvlock = (status & 0x01) > 0;
                let vbata = (status & 0x02) > 0;
                let ca_low = (status & 0x04) > 0;
                let ca_high = (status & 0x08) > 0;
                let accover = (status & 0x20) > 0;
                self.client.map(|client| {
                    client.status(uvlock, vbata, ca_low, ca_high, accover);
                });

                self.buffer.replace(buffer);
                self.i2c.disable();
                self.state.set(State::Idle);
            }
            State::ReadCharge => {
                // Charge is calculated in user space
                let charge = ((buffer[2] as u16) << 8) | (buffer[3] as u16);
                self.client.map(|client| {
                    client.charge(charge);
                });

                self.buffer.replace(buffer);
                self.i2c.disable();
                self.state.set(State::Idle);
            }
            State::ReadVoltage => {
                let voltage = ((buffer[8] as u16) << 8) | (buffer[9] as u16);
                self.client.map(|client| {
                    client.voltage(voltage);
                });

                self.buffer.replace(buffer);
                self.i2c.disable();
                self.state.set(State::Idle);
            }
            State::ReadCurrent => {
                let current = ((buffer[14] as u16) << 8) | (buffer[15] as u16);
                self.client.map(|client| {
                    client.current(current);
                });

                self.buffer.replace(buffer);
                self.i2c.disable();
                self.state.set(State::Idle);
            }
            State::ReadShutdown => {
                // Set the shutdown pin to 1
                buffer[1] |= 0x01;

                // Write the control register back but with a 1 in the shutdown
                // bit.
                buffer[0] = Registers::Control as u8;
                // TODO verify errors
                let _ = self.i2c.write(buffer, 2);
                self.state.set(State::Done);
            }
            State::Done => {
                self.client.map(|client| {
                    client.done();
                });

                self.buffer.replace(buffer);
                self.i2c.disable();
                self.state.set(State::Idle);
            }
            _ => {}
        }
    }
}

impl<I: i2c::I2CDevice> gpio::Client for LTC294X<'_, I> {
    fn fired(&self) {
        self.client.map(|client| {
            client.interrupt();
        });
    }
}

/// IDs for subscribed upcalls.
mod upcall {
    /// The callback that that is triggered when events finish and when readings
    /// are ready. The first argument represents which callback was triggered.
    ///
    /// - `0`: Interrupt occurred from the LTC294X.
    /// - `1`: Got the status.
    /// - `2`: Read the charge used.
    /// - `3`: `done()` was called.
    /// - `4`: Read the voltage.
    /// - `5`: Read the current.
    pub const EVENT_FINISHED: usize = 0;
    /// Number of upcalls.
    pub const COUNT: u8 = 1;
}

/// Default implementation of the LTC2941 driver that provides a Driver
/// interface for providing access to applications.
pub struct LTC294XDriver<'a, I: i2c::I2CDevice> {
    ltc294x: &'a LTC294X<'a, I>,
    grants: Grant<App, UpcallCount<{ upcall::COUNT }>, AllowRoCount<0>, AllowRwCount<0>>,
    owning_process: OptionalCell<ProcessId>,
}

impl<'a, I: i2c::I2CDevice> LTC294XDriver<'a, I> {
    pub fn new(
        ltc: &'a LTC294X<'a, I>,
        grants: Grant<App, UpcallCount<{ upcall::COUNT }>, AllowRoCount<0>, AllowRwCount<0>>,
    ) -> LTC294XDriver<'a, I> {
        LTC294XDriver {
            ltc294x: ltc,
            grants: grants,
            owning_process: OptionalCell::empty(),
        }
    }
}

impl<I: i2c::I2CDevice> LTC294XClient for LTC294XDriver<'_, I> {
    fn interrupt(&self) {
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(upcall::EVENT_FINISHED, (0, 0, 0))
                    .ok();
            });
        });
    }

    fn status(
        &self,
        undervolt_lockout: bool,
        vbat_alert: bool,
        charge_alert_low: bool,
        charge_alert_high: bool,
        accumulated_charge_overflow: bool,
    ) {
        let ret = (undervolt_lockout as usize)
            | ((vbat_alert as usize) << 1)
            | ((charge_alert_low as usize) << 2)
            | ((charge_alert_high as usize) << 3)
            | ((accumulated_charge_overflow as usize) << 4);
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(
                        upcall::EVENT_FINISHED,
                        (1, ret, self.ltc294x.model.get() as usize),
                    )
                    .ok();
            });
        });
    }

    fn charge(&self, charge: u16) {
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(upcall::EVENT_FINISHED, (2, charge as usize, 0))
                    .ok();
            });
        });
    }

    fn done(&self) {
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(upcall::EVENT_FINISHED, (3, 0, 0))
                    .ok();
            });
        });
    }

    fn voltage(&self, voltage: u16) {
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(upcall::EVENT_FINISHED, (4, voltage as usize, 0))
                    .ok();
            });
        });
    }

    fn current(&self, current: u16) {
        self.owning_process.map(|pid| {
            let _res = self.grants.enter(pid, |_app, upcalls| {
                upcalls
                    .schedule_upcall(upcall::EVENT_FINISHED, (5, current as usize, 0))
                    .ok();
            });
        });
    }
}

impl<I: i2c::I2CDevice> SyscallDriver for LTC294XDriver<'_, I> {
    /// Request operations for the LTC294X chip.
    ///
    /// ### `command_num`
    ///
    /// - `0`: Driver existence check.
    /// - `1`: Get status of the chip.
    /// - `2`: Configure settings of the chip.
    /// - `3`: Reset accumulated charge measurement to zero.
    /// - `4`: Set the upper threshold for charge.
    /// - `5`: Set the lower threshold for charge.
    /// - `6`: Get the current charge accumulated.
    /// - `7`: Shutdown the chip.
    /// - `8`: Get the voltage reading. Only supported on the LTC2942 and
    ///   LTC2943.
    /// - `9`: Get the current reading. Only supported on the LTC2943.
    /// - `10`: Set the model of the LTC294X actually being used. `data` is the
    ///   value of the X.
    fn command(
        &self,
        command_num: usize,
        data: usize,
        _: usize,
        process_id: ProcessId,
    ) -> CommandReturn {
        if command_num == 0 {
            // Handle this first as it should be returned
            // unconditionally
            return CommandReturn::success();
        }

        let match_or_empty_or_nonexistant = self.owning_process.map_or(true, |current_process| {
            self.grants
                .enter(current_process, |_, _| current_process == process_id)
                .unwrap_or(true)
        });
        if match_or_empty_or_nonexistant {
            self.owning_process.set(process_id);
        } else {
            return CommandReturn::failure(ErrorCode::NOMEM);
        }

        match command_num {
            // Get status.
            1 => self.ltc294x.read_status().into(),

            // Configure.
            2 => {
                let int_pin_raw = data & 0x03;
                let prescaler = (data >> 2) & 0x07;
                let vbat_raw = (data >> 5) & 0x03;
                let int_pin_conf = match int_pin_raw {
                    0 => InterruptPinConf::Disabled,
                    1 => InterruptPinConf::ChargeCompleteMode,
                    2 => InterruptPinConf::AlertMode,
                    _ => InterruptPinConf::Disabled,
                };
                let vbat_alert = match vbat_raw {
                    0 => VBatAlert::Off,
                    1 => VBatAlert::Threshold2V8,
                    2 => VBatAlert::Threshold2V9,
                    3 => VBatAlert::Threshold3V0,
                    _ => VBatAlert::Off,
                };

                self.ltc294x
                    .configure(int_pin_conf, prescaler as u8, vbat_alert)
                    .into()
            }

            // Reset charge.
            3 => self.ltc294x.reset_charge().into(),

            // Set high threshold
            4 => self.ltc294x.set_high_threshold(data as u16).into(),

            // Set low threshold
            5 => self.ltc294x.set_low_threshold(data as u16).into(),

            // Get charge
            6 => self.ltc294x.get_charge().into(),

            // Shutdown
            7 => self.ltc294x.shutdown().into(),

            // Get voltage
            8 => self.ltc294x.get_voltage().into(),

            // Get current
            9 => self.ltc294x.get_current().into(),

            // Set the current chip model
            10 => self.ltc294x.set_model(data).into(),

            // default
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.grants.enter(processid, |_, _| {})
    }
}
