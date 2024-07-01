// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the FM25CL FRAM chip.
//!
//! <http://www.cypress.com/part/fm25cl64b-dg>
//!
//! From the FM25CL website:
//!
//! > The FM25CL64B is a 64-Kbit nonvolatile memory employing an advanced
//! > ferroelectric process. A ferroelectric random access memory or F-RAM is
//! > nonvolatile and performs reads and writes similar to a RAM. It provides
//! > reliable data retention for 151 years while eliminating the complexities,
//! > overhead, and system level reliability problems caused by serial flash,
//! > EEPROM, and other nonvolatile memories.
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! // Create a SPI device for this chip.
//! let fm25cl_spi = static_init!(
//!     capsules::virtual_spi::VirtualSpiMasterDevice<'static, usart::USART>,
//!     capsules::virtual_spi::VirtualSpiMasterDevice::new(mux_spi, Some(&sam4l::gpio::PA[25])));
//! // Setup the actual FM25CL driver.
//! let fm25cl = static_init!(
//!     capsules::fm25cl::FM25CL<'static,
//!     capsules::virtual_spi::VirtualSpiMasterDevice<'static, usart::USART>>,
//!     capsules::fm25cl::FM25CL::new(fm25cl_spi,
//!         &mut capsules::fm25cl::TXBUFFER, &mut capsules::fm25cl::RXBUFFER));
//! fm25cl_spi.set_client(fm25cl);
//! ```
//!
//! This capsule provides two interfaces:
//!
//! - `hil::nonvolatile_storage::NonvolatileStorage`
//! - `FM25CLCustom`
//!
//! The first is the generic interface for nonvolatile storage. This allows
//! this driver to work with capsules like the `nonvolatile_storage_driver`
//! that provide virtualization and a userspace interface. The second is a
//! custom interface that exposes other chip-specific functions.

use core::cell::Cell;
use core::cmp;
use kernel::hil;
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::ErrorCode;

pub const BUF_LEN: usize = 512;

const SPI_SPEED: u32 = 4000000;

#[allow(dead_code)]
enum Opcodes {
    WriteEnable = 0x06,
    WriteDisable = 0x04,
    ReadStatusRegister = 0x05,
    WriteStatusRegister = 0x01,
    ReadMemory = 0x03,
    WriteMemory = 0x02,
}

#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,

    /// Simple read states
    ReadStatus,

    /// Write to the FRAM
    WriteEnable,
    WriteMemory,

    /// Read from the FRAM
    ReadMemory,
}

pub trait FM25CLCustom {
    fn read_status(&self) -> Result<(), ErrorCode>;
}

pub trait FM25CLClient {
    fn status(&self, status: u8);
    fn read(&self, data: &'static mut [u8], len: usize);
    fn done(&self, buffer: &'static mut [u8]);
}

pub struct FM25CL<'a, S: hil::spi::SpiMasterDevice<'a>> {
    spi: &'a S,
    state: Cell<State>,
    txbuffer: TakeCell<'static, [u8]>,
    rxbuffer: TakeCell<'static, [u8]>,
    client: OptionalCell<&'a dyn hil::nonvolatile_storage::NonvolatileStorageClient>,
    client_custom: OptionalCell<&'a dyn FM25CLClient>,
    client_buffer: TakeCell<'static, [u8]>, // Store buffer and state for passing back to client
    client_write_address: Cell<u16>,
    client_write_len: Cell<u16>,
}

impl<'a, S: hil::spi::SpiMasterDevice<'a>> FM25CL<'a, S> {
    pub fn new(
        spi: &'a S,
        txbuffer: &'static mut [u8],
        rxbuffer: &'static mut [u8],
    ) -> FM25CL<'a, S> {
        // setup and return struct
        FM25CL {
            spi: spi,
            state: Cell::new(State::Idle),
            txbuffer: TakeCell::new(txbuffer),
            rxbuffer: TakeCell::new(rxbuffer),
            client: OptionalCell::empty(),
            client_custom: OptionalCell::empty(),
            client_buffer: TakeCell::empty(),
            client_write_address: Cell::new(0),
            client_write_len: Cell::new(0),
        }
    }

    pub fn set_client<C: FM25CLClient>(&self, client: &'a C) {
        self.client_custom.set(client);
    }

    /// Setup SPI for this chip
    fn configure_spi(&self) -> Result<(), ErrorCode> {
        self.spi.configure(
            hil::spi::ClockPolarity::IdleLow,
            hil::spi::ClockPhase::SampleLeading,
            SPI_SPEED,
        )
    }

    pub fn write(
        &self,
        address: u16,
        buffer: &'static mut [u8],
        len: u16,
    ) -> Result<(), ErrorCode> {
        self.configure_spi()?;

        self.txbuffer
            .take()
            .map_or(Err(ErrorCode::RESERVE), move |txbuffer| {
                txbuffer[0] = Opcodes::WriteEnable as u8;

                let write_len = cmp::min(txbuffer.len(), len as usize);

                // Need to save the buffer passed to us so we can give it back.
                self.client_buffer.replace(buffer);
                // Also save address and len for the actual write.
                self.client_write_address.set(address);
                self.client_write_len.set(write_len as u16);

                self.state.set(State::WriteEnable);
                let res = self.spi.read_write_bytes(txbuffer, None, 1);
                match res {
                    Ok(()) => Ok(()),
                    Err((err, txbuffer, _)) => {
                        self.txbuffer.replace(txbuffer);
                        Err(err)
                    }
                }
            })
    }

    pub fn read(&self, address: u16, buffer: &'static mut [u8], len: u16) -> Result<(), ErrorCode> {
        self.configure_spi()?;

        self.txbuffer
            .take()
            .map_or(Err(ErrorCode::RESERVE), |txbuffer| {
                self.rxbuffer
                    .take()
                    .map_or(Err(ErrorCode::RESERVE), move |rxbuffer| {
                        txbuffer[0] = Opcodes::ReadMemory as u8;
                        txbuffer[1] = ((address >> 8) & 0xFF) as u8;
                        txbuffer[2] = (address & 0xFF) as u8;

                        // Save the user buffer for later
                        self.client_buffer.replace(buffer);

                        let read_len = cmp::min(rxbuffer.len() - 3, len as usize);

                        self.state.set(State::ReadMemory);
                        let res = self
                            .spi
                            .read_write_bytes(txbuffer, Some(rxbuffer), read_len + 3);
                        match res {
                            Ok(()) => Ok(()),
                            Err((err, txbuffer, rxbuffer)) => {
                                self.txbuffer.replace(txbuffer);
                                self.rxbuffer.replace(rxbuffer.unwrap());
                                Err(err)
                            }
                        }
                    })
            })
    }
}

impl<'a, S: hil::spi::SpiMasterDevice<'a>> hil::spi::SpiMasterClient for FM25CL<'a, S> {
    fn read_write_done(
        &self,
        write_buffer: &'static mut [u8],
        read_buffer: Option<&'static mut [u8]>,
        len: usize,
        _status: Result<(), ErrorCode>,
    ) {
        match self.state.get() {
            State::ReadStatus => {
                self.state.set(State::Idle);

                // Put back buffers that we got back from SPI layer.
                self.txbuffer.replace(write_buffer);

                read_buffer.map(|read_buffer| {
                    let status = read_buffer[1];

                    // Also replace this buffer
                    self.rxbuffer.replace(read_buffer);

                    self.client_custom.map(|client| client.status(status));
                });
            }
            State::WriteEnable => {
                self.state.set(State::WriteMemory);

                self.client_buffer.map(move |buffer| {
                    write_buffer[0] = Opcodes::WriteMemory as u8;
                    write_buffer[1] = ((self.client_write_address.get() >> 8) & 0xFF) as u8;
                    write_buffer[2] = (self.client_write_address.get() & 0xFF) as u8;

                    let write_len =
                        cmp::min(write_buffer.len(), self.client_write_len.get() as usize);

                    write_buffer[3..(write_len + 3)].copy_from_slice(&buffer[..write_len]);

                    let _ = self
                        .spi
                        .read_write_bytes(write_buffer, read_buffer, write_len + 3);
                });
            }
            State::WriteMemory => {
                self.state.set(State::Idle);

                let write_len = cmp::min(write_buffer.len(), self.client_write_len.get() as usize);

                // Replace these buffers
                self.txbuffer.replace(write_buffer);
                read_buffer.map(|read_buffer| {
                    self.rxbuffer.replace(read_buffer);
                });

                // Call done with the write() buffer
                self.client_buffer.take().map(move |buffer| {
                    self.client
                        .map(move |client| client.write_done(buffer, write_len));
                });
            }
            State::ReadMemory => {
                self.state.set(State::Idle);

                // Replace the TX buffer
                self.txbuffer.replace(write_buffer);

                read_buffer.map(|read_buffer| {
                    self.client_buffer.take().map(move |buffer| {
                        let read_len = cmp::min(buffer.len(), len);

                        buffer[..(read_len - 3)]
                            .copy_from_slice(&read_buffer[3..((read_len - 3) + 3)]);

                        self.rxbuffer.replace(read_buffer);

                        self.client
                            .map(move |client| client.read_done(buffer, read_len - 3));
                    });
                });
            }
            _ => {}
        }
    }
}

// Implement the custom interface that exposes chip-specific commands.
impl<'a, S: hil::spi::SpiMasterDevice<'a>> FM25CLCustom for FM25CL<'a, S> {
    fn read_status(&self) -> Result<(), ErrorCode> {
        self.configure_spi()?;

        self.txbuffer
            .take()
            .map_or(Err(ErrorCode::RESERVE), |txbuffer| {
                self.rxbuffer
                    .take()
                    .map_or(Err(ErrorCode::RESERVE), move |rxbuffer| {
                        txbuffer[0] = Opcodes::ReadStatusRegister as u8;

                        // Use 4 bytes instead of the required 2 because that works better
                        // with DMA for some reason.
                        // TODO verify SPI return value
                        let _ = self.spi.read_write_bytes(txbuffer, Some(rxbuffer), 4);
                        self.state.set(State::ReadStatus);
                        Ok(())
                    })
            })
    }
}

/// Implement the generic `NonvolatileStorage` interface common to chips that
/// provide nonvolatile memory.
impl<'a, S: hil::spi::SpiMasterDevice<'a>> hil::nonvolatile_storage::NonvolatileStorage<'a>
    for FM25CL<'a, S>
{
    fn set_client(&self, client: &'a dyn hil::nonvolatile_storage::NonvolatileStorageClient) {
        self.client.set(client);
    }

    fn read(
        &self,
        buffer: &'static mut [u8],
        address: usize,
        length: usize,
    ) -> Result<(), ErrorCode> {
        self.read(address as u16, buffer, length as u16)
    }

    fn write(
        &self,
        buffer: &'static mut [u8],
        address: usize,
        length: usize,
    ) -> Result<(), ErrorCode> {
        self.write(address as u16, buffer, length as u16)
    }
}
