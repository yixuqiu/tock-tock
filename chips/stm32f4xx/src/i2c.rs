// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

use core::cell::Cell;

use kernel::hil;
use kernel::hil::i2c::{self, Error, I2CHwMasterClient, I2CMaster};
use kernel::platform::chip::ClockInterface;
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::utilities::registers::interfaces::{ReadWriteable, Readable, Writeable};
use kernel::utilities::registers::{register_bitfields, ReadWrite};
use kernel::utilities::StaticRef;

use crate::clocks::{phclk, Stm32f4Clocks};

pub enum I2CSpeed {
    Speed100k,
    Speed400k,
}

/// Inter-Integrated Circuit
#[repr(C)]
struct I2CRegisters {
    /// control register 1
    cr1: ReadWrite<u32, CR1::Register>,
    /// control register 2
    cr2: ReadWrite<u32, CR2::Register>,
    /// own address register 1
    oar1: ReadWrite<u32, OAR1::Register>,
    /// own address register 2
    oar2: ReadWrite<u32, OAR2::Register>,
    /// data register
    dr: ReadWrite<u32, DR::Register>,
    /// status register 1
    sr1: ReadWrite<u32, SR1::Register>,
    /// status register 2
    sr2: ReadWrite<u32, SR2::Register>,
    /// clock control register
    ccr: ReadWrite<u32, CCR::Register>,
    /// tRise register
    trise: ReadWrite<u32, TRISE::Register>,
    /// filter register
    fltr: ReadWrite<u32, FLTR::Register>,
}

register_bitfields![u32,
    CR1 [
        /// Software reset
        SWRST OFFSET(15) NUMBITS(1) [],
        /// SMBus alert
        ALERT OFFSET(13) NUMBITS(1) [],
        /// Packet error checking
        PEC OFFSET(12) NUMBITS(1) [],
        /// Acknowledge/PEC Position (for data reception)
        POS OFFSET(11) NUMBITS(1) [],
        /// Acknowledge enable
        ACK OFFSET(10) NUMBITS(1) [],
        /// Stop generation
        STOP OFFSET(9) NUMBITS(1) [],
        /// Start generation
        START OFFSET(8) NUMBITS(1) [],
        /// Clock stretching disable (Slave mode)
        NOSTRETCH OFFSET(7) NUMBITS(1) [],
        /// General call enable
        ENGC OFFSET(6) NUMBITS(1) [],
        /// PEC enable
        ENPEC OFFSET(5) NUMBITS(1) [],
        /// ARP enable
        ENARP OFFSET(4) NUMBITS(1) [],
        /// SMBus type
        SMBTYPE OFFSET(3) NUMBITS(1) [],
        /// SMBus mode
        SMBUS OFFSET(1) NUMBITS(1) [],
        /// Peripheral enable
        PE OFFSET(0) NUMBITS(1) []
    ],
    CR2 [
        /// DMA last transfer
        LAST OFFSET(12) NUMBITS(1) [],
        /// DMA requests enable
        DMAEN OFFSET(11) NUMBITS(1) [],
        /// Buffer interrupt enable
        ITBUFEN OFFSET(10) NUMBITS(1) [],
        /// Event interrupt enable
        ITEVTEN OFFSET(9) NUMBITS(1) [],
        // Error interrupt enable
        ITERREN OFFSET(8) NUMBITS(1) [],
        /// Peripheral clock frequency
        FREQ OFFSET(0) NUMBITS(6) []
    ],
    OAR1 [
        /// Addressing mode (slave mode)
        ADDMODE OFFSET(15) NUMBITS(1) [],
        /// Interface address
        ADD OFFSET(0) NUMBITS(10) []
    ],
    OAR2 [
        /// Interface address
        ADD2 OFFSET(1) NUMBITS(7) [],
        /// Dual addressing mode enable
        ENDUAL OFFSET(1) NUMBITS(1) []
    ],
    DR [
        /// 8-bit receive data
        DR OFFSET(0) NUMBITS(8) []
    ],
    SR1 [
        /// SMBus alert
        SMBALERT OFFSET(15) NUMBITS(1) [],
        /// Timeout or tLOW detection flag
        TIMEOUT OFFSET(14) NUMBITS(1) [],
        /// PEC Error in reception
        PECERR OFFSET(12) NUMBITS(1) [],
        /// Overrun/Underrun
        OVR OFFSET(11) NUMBITS(1) [],
        /// Acknowledge failure
        AF OFFSET(10) NUMBITS(1) [],
        /// Arbitration lost
        ARLO OFFSET(9) NUMBITS(1) [],
        /// Bus error
        BERR OFFSET(8) NUMBITS(1) [],
        /// Data register empty (transmitters)
        TXE OFFSET(7) NUMBITS(1) [],
        /// Data register not empty (receivers)
        RXNE OFFSET(6) NUMBITS(1) [],
        /// Stop detection (slave mode)
        STOPF OFFSET(4) NUMBITS(1) [],
        /// 10-bit header sent (Master mode)
        ADD10 OFFSET(3) NUMBITS(1) [],
        /// Byte transfer finished
        BTF OFFSET(2) NUMBITS(1) [],
        /// Address sent (master mode)/matched (slave mode)
        ADDR OFFSET(1) NUMBITS(1) [],
        /// Start bit (Master mode)
        SB OFFSET(0) NUMBITS(1) []
    ],
    SR2 [
        /// Packet error checking register
        PEC OFFSET(8) NUMBITS(8) [],
        /// Dual flag (Slave mode)
        DUALF OFFSET(7) NUMBITS(1) [],
        /// SMBus host header (Slave mode)
        SMBHOST OFFSET(6) NUMBITS(1) [],
        /// SMBus device default address (Slave mode)
        SMBDEFAULT OFFSET(5) NUMBITS(1) [],
        /// General call address (Slave mode)
        GENCALL OFFSET(4) NUMBITS(1) [],
        /// Transmitter/receiver
        TRA OFFSET(2) NUMBITS(1) [],
        /// Bus busy
        BUSY OFFSET(1) NUMBITS(1) [],
        /// Master/slave
        MSL OFFSET(0) NUMBITS(1) []
    ],
    CCR [
        /// I2C master mode selection
        FS OFFSET(15) NUMBITS(1) [
            SM_MODE = 0,
            FM_MODE = 1
        ],
        /// Fm mode duty cycle
        DUTY OFFSET(14) NUMBITS(1) [],
        /// Clock control register in Fm/Sm mode (Master mode)
        CCR OFFSET(0) NUMBITS(12) []
    ],
    TRISE [
        /// Maximum rise time in Fm/Sm mode (Master mode)
        TRISE OFFSET(0) NUMBITS(6) []
    ],
    FLTR [
        /// Analog noise filter OFF
        ANOFF OFFSET(4) NUMBITS(1) [],
        /// Digital noise filter
        DNF OFFSET(0) NUMBITS(4) []
    ]
];

const I2C1_BASE: StaticRef<I2CRegisters> =
    unsafe { StaticRef::new(0x4000_5400 as *const I2CRegisters) };
// const I2C2_BASE: StaticRef<I2CRegisters> =
//     unsafe { StaticRef::new(0x4000_5800 as *const I2CRegisters) };
// const I2C3_BASE: StaticRef<I2CRegisters> =
//     unsafe { StaticRef::new(0x4000_5C00 as *const I2CRegisters) };

pub struct I2C<'a> {
    registers: StaticRef<I2CRegisters>,
    clock: I2CClock<'a>,

    // I2C slave support not yet implemented
    master_client: OptionalCell<&'a dyn hil::i2c::I2CHwMasterClient>,

    buffer: TakeCell<'static, [u8]>,
    tx_position: Cell<usize>,
    rx_position: Cell<usize>,
    tx_len: Cell<usize>,
    rx_len: Cell<usize>,

    slave_address: Cell<u8>,

    status: Cell<I2CStatus>,
}

#[derive(Copy, Clone, PartialEq)]
enum I2CStatus {
    Idle,
    Writing,
    WritingReading,
    Reading,
}

impl<'a> I2C<'a> {
    pub fn new(clocks: &'a dyn Stm32f4Clocks) -> Self {
        Self {
            registers: I2C1_BASE,
            clock: I2CClock(phclk::PeripheralClock::new(
                phclk::PeripheralClockType::APB1(phclk::PCLK1::I2C1),
                clocks,
            )),

            master_client: OptionalCell::empty(),

            slave_address: Cell::new(0),

            buffer: TakeCell::empty(),
            tx_position: Cell::new(0),
            rx_position: Cell::new(0),

            tx_len: Cell::new(0),
            rx_len: Cell::new(0),

            status: Cell::new(I2CStatus::Idle),
        }
    }

    pub fn set_speed(&self, speed: I2CSpeed, system_clock_in_mhz: usize) {
        self.disable();
        self.registers
            .cr2
            .modify(CR2::FREQ.val(system_clock_in_mhz as u32));
        match speed {
            I2CSpeed::Speed100k => {
                self.registers
                    .ccr
                    .modify(CCR::CCR.val(system_clock_in_mhz as u32 * 5) + CCR::FS::SM_MODE);
                self.registers
                    .trise
                    .modify(TRISE::TRISE.val(system_clock_in_mhz as u32 + 1));
            }
            I2CSpeed::Speed400k => {
                self.registers
                    .ccr
                    .modify(CCR::CCR.val(system_clock_in_mhz as u32 * 5 / 6) + CCR::FS::FM_MODE);
                self.registers
                    .trise
                    .modify(TRISE::TRISE.val(system_clock_in_mhz as u32 + 1));
            }
        }
        self.enable();
    }

    pub fn is_enabled_clock(&self) -> bool {
        self.clock.is_enabled()
    }

    pub fn enable_clock(&self) {
        self.clock.enable();
    }

    pub fn disable_clock(&self) {
        self.clock.disable();
    }

    pub fn handle_event(&self) {
        if self.registers.sr1.is_set(SR1::SB) {
            let dir = match self.status.get() {
                I2CStatus::Writing | I2CStatus::WritingReading => 0,
                I2CStatus::Reading => 1,
                _ => panic!("invalid i2c state when setting address"),
            };
            self.registers
                .dr
                .write(DR::DR.val(((self.slave_address.get() << 1) as u32) | dir));
        }
        if self.registers.sr1.is_set(SR1::ADDR) {
            // i2c requires a sr2 read
            self.registers.sr2.get();
        }
        if self.registers.sr1.is_set(SR1::TXE) {
            // send the next byte
            if self.buffer.is_some() && self.tx_position.get() < self.tx_len.get() {
                self.buffer.map(|buf| {
                    let byte = buf[self.tx_position.get()];
                    self.registers.dr.write(DR::DR.val(byte as u32));
                    self.tx_position.set(self.tx_position.get() + 1);
                });
            }
        }

        while self.registers.sr1.is_set(SR1::RXNE) {
            // send the next byte
            let byte = self.registers.dr.read(DR::DR);
            if self.buffer.is_some() && self.rx_position.get() < self.rx_len.get() {
                self.buffer.map(|buf| {
                    buf[self.rx_position.get()] = byte as u8;
                    self.rx_position.set(self.rx_position.get() + 1);
                });
            }

            if self.buffer.is_some() && self.rx_position.get() == self.rx_len.get() {
                self.registers.cr1.modify(CR1::STOP::SET);
                self.stop();
                self.master_client.map(|client| {
                    self.buffer
                        .take()
                        .map(|buf| client.command_complete(buf, Ok(())))
                });
            }
        }

        if self.registers.sr1.is_set(SR1::BTF) {
            match self.status.get() {
                I2CStatus::Writing | I2CStatus::WritingReading => {
                    if self.tx_position.get() < self.tx_len.get() {
                        self.registers.cr1.modify(CR1::STOP::SET);
                        self.stop();
                        self.master_client.map(|client| {
                            self.buffer
                                .take()
                                .map(|buf| client.command_complete(buf, Err(Error::DataNak)))
                        });
                    } else {
                        if self.status.get() == I2CStatus::Writing {
                            self.registers.cr1.modify(CR1::STOP::SET);
                            self.stop();
                            self.master_client.map(|client| {
                                self.buffer
                                    .take()
                                    .map(|buf| client.command_complete(buf, Ok(())))
                            });
                        } else {
                            self.status.set(I2CStatus::Reading);
                            self.start_read();
                        }
                    }
                }
                I2CStatus::Reading => {
                    let status = if self.rx_position.get() == self.rx_len.get() {
                        Ok(())
                    } else {
                        Err(Error::DataNak)
                    };
                    self.registers.cr1.modify(CR1::STOP::SET);
                    self.stop();
                    self.master_client.map(|client| {
                        self.buffer
                            .take()
                            .map(|buf| client.command_complete(buf, status))
                    });
                }
                _ => panic!("i2c status error"),
            }
        }
    }

    pub fn handle_error(&self) {
        self.master_client.map(|client| {
            self.buffer
                .take()
                .map(|buf| client.command_complete(buf, Err(Error::DataNak)))
        });
        self.stop();
    }

    fn reset(&self) {
        self.disable();
        self.enable();
    }

    fn start_write(&self) {
        self.tx_position.set(0);
        self.registers
            .cr2
            .modify(CR2::ITEVTEN::SET + CR2::ITERREN::SET + CR2::ITBUFEN::SET);
        self.registers.cr1.modify(CR1::ACK::SET);
        self.registers.cr1.modify(CR1::START::SET);
    }

    fn stop(&self) {
        self.registers
            .cr2
            .modify(CR2::ITEVTEN::CLEAR + CR2::ITERREN::CLEAR + CR2::ITBUFEN::CLEAR);
        self.registers.cr1.modify(CR1::ACK::CLEAR);
        self.status.set(I2CStatus::Idle);
    }

    fn start_read(&self) {
        self.rx_position.set(0);
        self.registers
            .cr2
            .modify(CR2::ITEVTEN::SET + CR2::ITERREN::SET + CR2::ITBUFEN::SET);
        self.registers.cr1.modify(CR1::ACK::SET);
        self.registers.cr1.modify(CR1::START::SET);
    }
}

impl<'a> i2c::I2CMaster<'a> for I2C<'a> {
    fn set_master_client(&self, master_client: &'a dyn I2CHwMasterClient) {
        self.master_client.replace(master_client);
    }
    fn enable(&self) {
        self.registers.cr1.modify(CR1::PE::SET);
    }
    fn disable(&self) {
        self.registers.cr1.modify(CR1::PE::CLEAR);
    }
    fn write_read(
        &self,
        addr: u8,
        data: &'static mut [u8],
        write_len: usize,
        read_len: usize,
    ) -> Result<(), (Error, &'static mut [u8])> {
        if self.status.get() == I2CStatus::Idle {
            self.reset();
            self.status.set(I2CStatus::WritingReading);
            self.slave_address.set(addr);
            self.buffer.replace(data);
            self.tx_len.set(write_len);
            self.rx_len.set(read_len);
            self.start_write();
            Ok(())
        } else {
            Err((Error::Busy, data))
        }
    }
    fn write(
        &self,
        addr: u8,
        data: &'static mut [u8],
        len: usize,
    ) -> Result<(), (Error, &'static mut [u8])> {
        if self.status.get() == I2CStatus::Idle {
            self.reset();
            self.status.set(I2CStatus::Writing);
            self.slave_address.set(addr);
            self.buffer.replace(data);
            self.tx_len.set(len);
            self.start_write();
            Ok(())
        } else {
            Err((Error::Busy, data))
        }
    }
    fn read(
        &self,
        addr: u8,
        buffer: &'static mut [u8],
        len: usize,
    ) -> Result<(), (Error, &'static mut [u8])> {
        if self.status.get() == I2CStatus::Idle {
            self.reset();
            self.status.set(I2CStatus::Reading);
            self.slave_address.set(addr);
            self.buffer.replace(buffer);
            self.rx_len.set(len);
            self.start_read();
            Ok(())
        } else {
            Err((Error::ArbitrationLost, buffer))
        }
    }
}

struct I2CClock<'a>(phclk::PeripheralClock<'a>);

impl ClockInterface for I2CClock<'_> {
    fn is_enabled(&self) -> bool {
        self.0.is_enabled()
    }

    fn enable(&self) {
        self.0.enable();
    }

    fn disable(&self) {
        self.0.disable();
    }
}
