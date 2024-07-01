// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the MLX90614 Infrared Thermometer.
//!
//! SMBus Interface
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! let mux_i2c = components::i2c::I2CMuxComponent::new(&earlgrey::i2c::I2C)
//!     .finalize(components::i2c_mux_component_helper!());
//!
//! let mlx90614 = components::mlx90614::Mlx90614I2CComponent::new()
//!    .finalize(components::mlx90614_i2c_component_helper!(mux_i2c));
//! ```
//!

use core::cell::Cell;

use enum_primitive::cast::FromPrimitive;
use enum_primitive::enum_from_primitive;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil::i2c;
use kernel::hil::sensors;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::utilities::registers::register_bitfields;
use kernel::{ErrorCode, ProcessId};

use capsules_core::driver;

/// Syscall driver number.
pub const DRIVER_NUM: usize = driver::NUM::Mlx90614 as usize;

register_bitfields![u16,
    CONFIG [
        IIR OFFSET(0) NUMBITS(3) [],
        DUAL OFFSET(6) NUMBITS(1) [],
        FIR OFFSET(8) NUMBITS(3) [],
        GAIN OFFSET(11) NUMBITS(3) []
    ]
];

#[derive(Clone, Copy, PartialEq, Debug)]
enum State {
    Idle,
    IsPresent,
    ReadAmbientTemp,
    ReadObjTemp,
}

enum_from_primitive! {
    enum Mlx90614Registers {
        RAW1 = 0x04,
        RAW2 = 0x05,
        TA = 0x06,
        TOBJ1 = 0x07,
        TOBJ2 = 0x08,
        EMISSIVITY = 0x24,
        CONFIG = 0x25,
    }
}

#[derive(Default)]
pub struct App {}

pub struct Mlx90614SMBus<'a, S: i2c::SMBusDevice> {
    smbus_temp: &'a S,
    temperature_client: OptionalCell<&'a dyn sensors::TemperatureClient>,
    buffer: TakeCell<'static, [u8]>,
    state: Cell<State>,
    apps: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    owning_process: OptionalCell<ProcessId>,
}

impl<'a, S: i2c::SMBusDevice> Mlx90614SMBus<'a, S> {
    pub fn new(
        smbus_temp: &'a S,
        buffer: &'static mut [u8],
        grant: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    ) -> Mlx90614SMBus<'a, S> {
        Mlx90614SMBus {
            smbus_temp,
            temperature_client: OptionalCell::empty(),
            buffer: TakeCell::new(buffer),
            state: Cell::new(State::Idle),
            apps: grant,
            owning_process: OptionalCell::empty(),
        }
    }

    fn is_present(&self) {
        self.state.set(State::IsPresent);
        self.buffer.take().map(|buf| {
            // turn on i2c to send commands
            buf[0] = Mlx90614Registers::RAW1 as u8;
            self.smbus_temp.smbus_write_read(buf, 1, 1).unwrap();
        });
    }

    fn read_ambient_temperature(&self) {
        self.state.set(State::ReadAmbientTemp);
        self.buffer.take().map(|buf| {
            buf[0] = Mlx90614Registers::TA as u8;
            self.smbus_temp.smbus_write_read(buf, 1, 1).unwrap();
        });
    }

    fn read_object_temperature(&self) {
        self.state.set(State::ReadObjTemp);
        self.buffer.take().map(|buf| {
            buf[0] = Mlx90614Registers::TOBJ1 as u8;
            self.smbus_temp.smbus_write_read(buf, 1, 2).unwrap();
        });
    }
}

impl<'a, S: i2c::SMBusDevice> i2c::I2CClient for Mlx90614SMBus<'a, S> {
    fn command_complete(&self, buffer: &'static mut [u8], status: Result<(), i2c::Error>) {
        match self.state.get() {
            State::Idle => {
                self.buffer.replace(buffer);
            }
            State::IsPresent => {
                let present = status.is_ok() && buffer[0] == 60;

                self.owning_process.map(|pid| {
                    let _ = self.apps.enter(pid, |_app, upcalls| {
                        upcalls
                            .schedule_upcall(0, (usize::from(present), 0, 0))
                            .ok();
                    });
                });
                self.buffer.replace(buffer);
                self.state.set(State::Idle);
            }
            State::ReadAmbientTemp | State::ReadObjTemp => {
                let values = match status {
                    Ok(()) =>
                    // Convert to centi celsius
                    {
                        Ok(((buffer[0] as usize | (buffer[1] as usize) << 8) * 2) as i32 - 27300)
                    }
                    Err(i2c_error) => Err(i2c_error.into()),
                };
                self.temperature_client.map(|client| {
                    client.callback(values);
                });
                if let Ok(temp) = values {
                    self.owning_process.map(|pid| {
                        let _ = self.apps.enter(pid, |_app, upcalls| {
                            upcalls.schedule_upcall(0, (temp as usize, 0, 0)).ok();
                        });
                    });
                } else {
                    self.owning_process.map(|pid| {
                        let _ = self.apps.enter(pid, |_app, upcalls| {
                            upcalls.schedule_upcall(0, (0, 0, 0)).ok();
                        });
                    });
                }
                self.buffer.replace(buffer);
                self.state.set(State::Idle);
            }
        }
    }
}

impl<'a, S: i2c::SMBusDevice> SyscallDriver for Mlx90614SMBus<'a, S> {
    fn command(
        &self,
        command_num: usize,
        _data1: usize,
        _data2: usize,
        process_id: ProcessId,
    ) -> CommandReturn {
        if command_num == 0 {
            // Handle this first as it should be returned
            // unconditionally
            return CommandReturn::success();
        }
        // Check if this non-virtualized driver is already in use by
        // some (alive) process
        let match_or_empty_or_nonexistant = self.owning_process.map_or(true, |current_process| {
            self.apps
                .enter(current_process, |_, _| current_process == process_id)
                .unwrap_or(true)
        });
        if match_or_empty_or_nonexistant {
            self.owning_process.set(process_id);
        } else {
            return CommandReturn::failure(ErrorCode::NOMEM);
        }

        match command_num {
            0 => CommandReturn::success(),
            // Check is sensor is correctly connected
            1 => {
                if self.state.get() == State::Idle {
                    self.is_present();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Read Ambient Temperature
            2 => {
                if self.state.get() == State::Idle {
                    self.read_ambient_temperature();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Read Object Temperature
            3 => {
                if self.state.get() == State::Idle {
                    self.read_object_temperature();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // default
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.apps.enter(processid, |_, _| {})
    }
}

impl<'a, S: i2c::SMBusDevice> sensors::TemperatureDriver<'a> for Mlx90614SMBus<'a, S> {
    fn set_client(&self, temperature_client: &'a dyn sensors::TemperatureClient) {
        self.temperature_client.replace(temperature_client);
    }

    fn read_temperature(&self) -> Result<(), ErrorCode> {
        self.read_object_temperature();
        Ok(())
    }
}
