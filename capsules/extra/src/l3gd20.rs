// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the MEMS L3gd20Spi motion sensor, 3 axys digital output gyroscope
//! and temperature sensor.
//!
//! May be used with NineDof and Temperature
//!
//! SPI Interface
//!
//! <https://www.pololu.com/file/0J563/L3gd20Spi.pdf>
//!
//!
//! Syscall Interface
//! -----------------
//!
//! ### Command
//!
//! All commands are asynchronous, they return a one shot callback when done
//! Only one command can be issued at a time.
//!
//! #### command num
//! - `0`: Returns Ok(())
//!   - `data`: Unused.
//!   - Return: Success
//! - `1`: Is Present
//!   - `data`: unused
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `2`: Power On
//!   - `data`: unused
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `3`: Set Scale
//!   - `data1`: 0, 1 or 2
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `4`: Enable high pass filter
//!   - `data`: 1 for enable, 0 for disable
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `5`: Set High Pass Filter Mode and Divider (manual page 33)
//!   - `data1`: mode
//!   - `data2`: divider
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `6`: Read XYZ
//!   - `data`: unused
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//! - `7`: Read Temperature
//!   - `data`: unused
//!   - Return: `Ok(())` if no other command is in progress, `BUSY` otherwise.
//!
//! ### Subscribe
//!
//! All commands call this callback when done, usually subscribes
//! should be one time functions
//!
//! #### subscribe num
//! - `0`: Done callback
//!   - 'data1`: depends on command
//!     - `1` - 1 for is present, 0 for not present
//!     - `6` - X rotation
//!     - `7` - temperature in deg C
//!   - 'data2`: depends on command
//!     - `6` - Y rotation
//!   - 'data3`: depends on command
//!     - `6` - Z rotation
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! let mux_spi = components::spi::SpiMuxComponent::new(&stm32f3xx::spi::SPI1)
//!     .finalize(components::spi_mux_component_helper!(stm32f3xx::spi::Spi));
//!
//! let l3gd20 = components::l3gd20::L3gd20SpiComponent::new()
//!     .finalize(components::l3gd20_spi_component_helper!(stm32f3xx::spi::Spi, stm32f3xx::gpio::PinId::PE03, mux_spi));
//!
//! ```
//!
//! NineDof Example
//!
//! ```rust,ignore
//! let grant_cap = create_capability!(capabilities::MemoryAllocationCapability);
//! let grant_ninedof = board_kernel.create_grant(&grant_cap);
//!
//! l3gd20.power_on();
//! let ninedof = static_init!(
//!     capsules::ninedof::NineDof<'static>,
//!     capsules::ninedof::NineDof::new(l3gd20, grant_ninedof));
//! hil::sensors::NineDof::set_client(l3gd20, ninedof);
//!
//! ```
//!
//! Temperature Example
//!
//! ```rust,ignore
//! let grant_cap = create_capability!(capabilities::MemoryAllocationCapability);
//! let grant_temp = board_kernel.create_grant(&grant_cap);
//!
//! l3gd20.power_on();
//! let temp = static_init!(
//! capsules::temperature::TemperatureSensor<'static>,
//!     capsules::temperature::TemperatureSensor::new(l3gd20, grant_temperature));
//! kernel::hil::sensors::TemperatureDriver::set_client(l3gd20, temp);
//!
//! ```
//!
//! Author: Alexandru Radovici <msg4alex@gmail.com>
//!

use core::cell::Cell;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil::sensors;
use kernel::hil::spi;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::{ErrorCode, ProcessId};

use capsules_core::driver;
pub const DRIVER_NUM: usize = driver::NUM::L3gd20 as usize;

/* Identification number */
const L3GD20_WHO_AM_I: u8 = 0xD4;

/* Registers addresses */
const L3GD20_REG_WHO_AM_I: u8 = 0x0F;
const L3GD20_REG_CTRL_REG1: u8 = 0x20;
const L3GD20_REG_CTRL_REG2: u8 = 0x21;
// const L3GD20_REG_CTRL_REG3: u8 = 0x22;
const L3GD20_REG_CTRL_REG4: u8 = 0x23;
const L3GD20_REG_CTRL_REG5: u8 = 0x24;
// const L3GD20_REG_REFERENCE: u8 = 0x25;
const L3GD20_REG_OUT_TEMP: u8 = 0x26;
// const L3GD20_REG_STATUS_REG: u8 = 0x27;
const L3GD20_REG_OUT_X_L: u8 = 0x28;
/*
const L3GD20_REG_OUT_X_H: u8 = 0x29;
const L3GD20_REG_OUT_Y_L: u8 = 0x2A;
const L3GD20_REG_OUT_Y_H: u8 = 0x2B;
const L3GD20_REG_OUT_Z_L: u8 = 0x2C;
const L3GD20_REG_OUT_Z_H: u8 = 0x2D;
*/
/*
const L3GD20_REG_FIFO_CTRL_REG: u8 = 0x2E;
const L3GD20_REG_FIFO_SRC_REG: u8 = 0x2F;
const L3GD20_REG_INT1_CFG: u8 = 0x30;
const L3GD20_REG_INT1_SRC: u8 = 0x31;
const L3GD20_REG_INT1_TSH_XH: u8 = 0x32;
const L3GD20_REG_INT1_TSH_XL: u8 = 0x33;
const L3GD20_REG_INT1_TSH_YH: u8 = 0x34;
const L3GD20_REG_INT1_TSH_YL: u8 = 0x35;
const L3GD20_REG_INT1_TSH_ZH: u8 = 0x36;
const L3GD20_REG_INT1_TSH_ZL: u8 = 0x37;
const L3GD20_REG_INT1_DURATION: u8 = 0x38;
*/

pub const L3GD20_TX_SIZE: usize = 10;
pub const L3GD20_RX_SIZE: usize = 10;

pub const TX_BUF_LEN: usize = L3GD20_TX_SIZE;
pub const RX_BUF_LEN: usize = L3GD20_RX_SIZE;

/* Sensitivity factors, datasheet pg. 9 */
const L3GD20_SCALE_250: isize = 875; /* 8.75 mdps/digit */
const L3GD20_SCALE_500: isize = 1750; /* 17.5 mdps/digit */
const L3GD20_SCALE_2000: isize = 7000; /* 70 mdps/digit */

#[derive(Copy, Clone, PartialEq)]
enum L3gd20Status {
    Idle,
    IsPresent,
    PowerOn,
    EnableHpf,
    SetHpfParameters,
    SetScale,
    ReadXYZ,
    ReadTemperature,
}

// #[derive(Clone, Copy, PartialEq)]
// enum L3GD20State {
//     Idle,
// }

#[derive(Default)]
pub struct App {}

pub struct L3gd20Spi<'a, S: spi::SpiMasterDevice<'a>> {
    spi: &'a S,
    txbuffer: TakeCell<'static, [u8]>,
    rxbuffer: TakeCell<'static, [u8]>,
    status: Cell<L3gd20Status>,
    hpf_enabled: Cell<bool>,
    hpf_mode: Cell<u8>,
    hpf_divider: Cell<u8>,
    scale: Cell<u8>,
    current_process: OptionalCell<ProcessId>,
    grants: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    nine_dof_client: OptionalCell<&'a dyn sensors::NineDofClient>,
    temperature_client: OptionalCell<&'a dyn sensors::TemperatureClient>,
}

impl<'a, S: spi::SpiMasterDevice<'a>> L3gd20Spi<'a, S> {
    pub fn new(
        spi: &'a S,
        txbuffer: &'static mut [u8; L3GD20_TX_SIZE],
        rxbuffer: &'static mut [u8; L3GD20_RX_SIZE],
        grants: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    ) -> L3gd20Spi<'a, S> {
        // setup and return struct
        L3gd20Spi {
            spi: spi,
            txbuffer: TakeCell::new(txbuffer),
            rxbuffer: TakeCell::new(rxbuffer),
            status: Cell::new(L3gd20Status::Idle),
            hpf_enabled: Cell::new(false),
            hpf_mode: Cell::new(0),
            hpf_divider: Cell::new(0),
            scale: Cell::new(0),
            current_process: OptionalCell::empty(),
            grants: grants,
            nine_dof_client: OptionalCell::empty(),
            temperature_client: OptionalCell::empty(),
        }
    }

    pub fn is_present(&self) -> bool {
        self.status.set(L3gd20Status::IsPresent);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_WHO_AM_I | 0x80;
            buf[1] = 0x00;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, self.rxbuffer.take(), 2);
        });
        false
    }

    pub fn power_on(&self) {
        self.status.set(L3gd20Status::PowerOn);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_CTRL_REG1;
            buf[1] = 0x0F;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, None, 2);
        });
    }

    fn enable_hpf(&self, enabled: bool) {
        self.status.set(L3gd20Status::EnableHpf);
        self.hpf_enabled.set(enabled);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_CTRL_REG5;
            buf[1] = u8::from(enabled) << 4;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, None, 2);
        });
    }

    fn set_hpf_parameters(&self, mode: u8, divider: u8) {
        self.status.set(L3gd20Status::SetHpfParameters);
        self.hpf_mode.set(mode);
        self.hpf_divider.set(divider);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_CTRL_REG2;
            buf[1] = (mode & 0x03) << 4 | (divider & 0x0F);
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, None, 2);
        });
    }

    fn set_scale(&self, scale: u8) {
        self.status.set(L3gd20Status::SetScale);
        self.scale.set(scale);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_CTRL_REG4;
            buf[1] = (scale & 0x03) << 4;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, None, 2);
        });
    }

    fn read_xyz(&self) {
        self.status.set(L3gd20Status::ReadXYZ);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_OUT_X_L | 0x80 | 0x40;
            buf[1] = 0x00;
            buf[2] = 0x00;
            buf[3] = 0x00;
            buf[4] = 0x00;
            buf[5] = 0x00;
            buf[6] = 0x00;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, self.rxbuffer.take(), 7);
        });
    }

    fn read_temperature(&self) {
        self.status.set(L3gd20Status::ReadTemperature);
        self.txbuffer.take().map(|buf| {
            buf[0] = L3GD20_REG_OUT_TEMP | 0x80;
            buf[1] = 0x00;
            // TODO verify SPI return value
            let _ = self.spi.read_write_bytes(buf, self.rxbuffer.take(), 2);
        });
    }

    pub fn configure(&self) -> Result<(), ErrorCode> {
        self.spi.configure(
            spi::ClockPolarity::IdleHigh,
            spi::ClockPhase::SampleTrailing,
            1_000_000,
        )
    }
}

impl<'a, S: spi::SpiMasterDevice<'a>> SyscallDriver for L3gd20Spi<'a, S> {
    fn command(
        &self,
        command_num: usize,
        data1: usize,
        data2: usize,
        process_id: ProcessId,
    ) -> CommandReturn {
        if command_num == 0 {
            return CommandReturn::success();
        }

        let match_or_empty_or_nonexistent = self.current_process.map_or(true, |current_process| {
            self.grants
                .enter(current_process, |_, _| current_process == process_id)
                .unwrap_or(true)
        });

        if match_or_empty_or_nonexistent {
            self.current_process.set(process_id);
        } else {
            return CommandReturn::failure(ErrorCode::RESERVE);
        }

        match command_num {
            // Check is sensor is correctly connected
            1 => {
                if self.status.get() == L3gd20Status::Idle {
                    self.is_present();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Power On
            2 => {
                if self.status.get() == L3gd20Status::Idle {
                    self.power_on();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Set Scale
            3 => {
                if self.status.get() == L3gd20Status::Idle {
                    let scale = data1 as u8;
                    self.set_scale(scale);
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Enable High Pass Filter
            4 => {
                if self.status.get() == L3gd20Status::Idle {
                    let mode = data1 as u8;
                    let divider = data2 as u8;
                    self.set_hpf_parameters(mode, divider);
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Set High Pass Filter Mode and Divider
            5 => {
                if self.status.get() == L3gd20Status::Idle {
                    let enabled = data1 == 1;
                    self.enable_hpf(enabled);
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Read XYZ
            6 => {
                if self.status.get() == L3gd20Status::Idle {
                    self.read_xyz();
                    CommandReturn::success()
                } else {
                    CommandReturn::failure(ErrorCode::BUSY)
                }
            }
            // Read Temperature
            7 => {
                if self.status.get() == L3gd20Status::Idle {
                    self.read_temperature();
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
        self.grants.enter(processid, |_, _| {})
    }
}

impl<'a, S: spi::SpiMasterDevice<'a>> spi::SpiMasterClient for L3gd20Spi<'a, S> {
    fn read_write_done(
        &self,
        write_buffer: &'static mut [u8],
        read_buffer: Option<&'static mut [u8]>,
        len: usize,
        _status: Result<(), ErrorCode>,
    ) {
        self.current_process.map(|proc_id| {
            let _result = self.grants.enter(proc_id, |_app, upcalls| {
                self.status.set(match self.status.get() {
                    L3gd20Status::IsPresent => {
                        let present = if let Some(ref buf) = read_buffer {
                            buf[1] == L3GD20_WHO_AM_I
                        } else {
                            false
                        };
                        upcalls
                            .schedule_upcall(0, (1, usize::from(present), 0))
                            .ok();
                        L3gd20Status::Idle
                    }

                    L3gd20Status::ReadXYZ => {
                        let mut x: usize = 0;
                        let mut y: usize = 0;
                        let mut z: usize = 0;
                        let values = if let Some(ref buf) = read_buffer {
                            if len >= 7 {
                                self.nine_dof_client.map(|client| {
                                    // compute using only integers
                                    let scale = match self.scale.get() {
                                        0 => L3GD20_SCALE_250,
                                        1 => L3GD20_SCALE_500,
                                        _ => L3GD20_SCALE_2000,
                                    };
                                    let x: usize =
                                        ((buf[1] as i16 | ((buf[2] as i16) << 8)) as isize * scale
                                            / 100000)
                                            as usize;
                                    let y: usize =
                                        ((buf[3] as i16 | ((buf[4] as i16) << 8)) as isize * scale
                                            / 100000)
                                            as usize;
                                    let z: usize =
                                        ((buf[5] as i16 | ((buf[6] as i16) << 8)) as isize * scale
                                            / 100000)
                                            as usize;
                                    client.callback(x, y, z);
                                });
                                // actual computation is this one

                                x = (buf[1] as i16 | ((buf[2] as i16) << 8)) as usize;
                                y = (buf[3] as i16 | ((buf[4] as i16) << 8)) as usize;
                                z = (buf[5] as i16 | ((buf[6] as i16) << 8)) as usize;
                                true
                            } else {
                                self.nine_dof_client.map(|client| {
                                    client.callback(0, 0, 0);
                                });
                                false
                            }
                        } else {
                            false
                        };
                        if values {
                            upcalls.schedule_upcall(0, (x, y, z)).ok();
                        } else {
                            upcalls.schedule_upcall(0, (0, 0, 0)).ok();
                        }
                        L3gd20Status::Idle
                    }

                    L3gd20Status::ReadTemperature => {
                        let mut temperature = 0;
                        let value = if let Some(ref buf) = read_buffer {
                            if len >= 2 {
                                temperature = buf[1] as i32;
                                self.temperature_client.map(|client| {
                                    client.callback(Ok(temperature * 100));
                                });
                                true
                            } else {
                                self.temperature_client.map(|client| {
                                    client.callback(Err(ErrorCode::FAIL));
                                });
                                false
                            }
                        } else {
                            false
                        };
                        if value {
                            upcalls
                                .schedule_upcall(0, (temperature as usize, 0, 0))
                                .ok();
                        } else {
                            upcalls.schedule_upcall(0, (0, 0, 0)).ok();
                        }
                        L3gd20Status::Idle
                    }

                    _ => {
                        upcalls.schedule_upcall(0, (0, 0, 0)).ok();
                        L3gd20Status::Idle
                    }
                });
            });
        });
        self.txbuffer.replace(write_buffer);
        if let Some(buf) = read_buffer {
            self.rxbuffer.replace(buf);
        }
    }
}

impl<'a, S: spi::SpiMasterDevice<'a>> sensors::NineDof<'a> for L3gd20Spi<'a, S> {
    fn set_client(&self, nine_dof_client: &'a dyn sensors::NineDofClient) {
        self.nine_dof_client.replace(nine_dof_client);
    }

    fn read_gyroscope(&self) -> Result<(), ErrorCode> {
        if self.status.get() == L3gd20Status::Idle {
            self.read_xyz();
            Ok(())
        } else {
            Err(ErrorCode::BUSY)
        }
    }
}

impl<'a, S: spi::SpiMasterDevice<'a>> sensors::TemperatureDriver<'a> for L3gd20Spi<'a, S> {
    fn set_client(&self, temperature_client: &'a dyn sensors::TemperatureClient) {
        self.temperature_client.replace(temperature_client);
    }

    fn read_temperature(&self) -> Result<(), ErrorCode> {
        if self.status.get() == L3gd20Status::Idle {
            self.read_temperature();
            Ok(())
        } else {
            Err(ErrorCode::BUSY)
        }
    }
}
