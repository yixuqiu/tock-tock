// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

use crate::clocks::{phclk, Stm32f4Clocks};
use core::cell::Cell;
use kernel::deferred_call::{DeferredCall, DeferredCallClient};
use kernel::hil::bus8080::{Bus8080, BusWidth, Client};
use kernel::platform::chip::ClockInterface;
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::utilities::registers::interfaces::{ReadWriteable, Readable};
use kernel::utilities::registers::{register_bitfields, ReadWrite};
use kernel::utilities::StaticRef;
use kernel::ErrorCode;

/// FSMC peripheral interface
#[repr(C)]
struct FsmcBankRegisters {
    /// SRAM/NOR-Flash chip-select control register
    bcr1: ReadWrite<u32, BCR::Register>,
    /// SRAM/NOR-Flash chip-select timing register
    btr1: ReadWrite<u32, BTR::Register>,
    /// SRAM/NOR-Flash chip-select control register
    bcr2: ReadWrite<u32, BCR::Register>,
    /// SRAM/NOR-Flash chip-select timing register
    btr2: ReadWrite<u32, BTR::Register>,
    /// SRAM/NOR-Flash chip-select control register
    bcr3: ReadWrite<u32, BCR::Register>,
    /// SRAM/NOR-Flash chip-select timing register
    btr3: ReadWrite<u32, BTR::Register>,
    /// SRAM/NOR-Flash chip-select control register
    bcr4: ReadWrite<u32, BCR::Register>,
    /// SRAM/NOR-Flash chip-select timing register
    btr4: ReadWrite<u32, BTR::Register>,
    _reseved: [u8; 228],
    /// SRAM/NOR-Flash write timing registers
    bwtr1: ReadWrite<u32, BWTR::Register>,
    /// SRAM/NOR-Flash write timing registers
    bwtr2: ReadWrite<u32, BWTR::Register>,
    /// SRAM/NOR-Flash write timing registers
    bwtr3: ReadWrite<u32, BWTR::Register>,
    /// SRAM/NOR-Flash write timing registers
    bwtr4: ReadWrite<u32, BWTR::Register>,
}

register_bitfields![u32,
    BCR [
        /// Write FIFO Disable
        WFDIS OFFSET(21) NUMBITS(1) [],
        /// Continuous Clock Enable
        CCLKEN OFFSET(20) NUMBITS(1) [],
        /// Write burst enable
        CBURSTRW OFFSET(19) NUMBITS(1) [],
        /// CRAM page size
        CPSIZE OFFSET(16) NUMBITS(3) [
            NO_BURST = 0b000,
            BYTES_128 = 0b001,
            BYTES_256 = 0b010,
            BYTES_1024 = 0b011
        ],
        /// Wait signal during asynchronous transfers
        ASYNCWAIT OFFSET(15) NUMBITS(1) [],
        /// Extended mode enable
        EXTMOD OFFSET(14) NUMBITS(1) [],
        /// Wait enable bit
        WAITEN OFFSET(13) NUMBITS(1) [],
        /// Write enable bit
        WREN OFFSET(12) NUMBITS(1) [],
        /// Wait timing configuration
        WAITCFG OFFSET(11) NUMBITS(1) [],
        /// Wait signal polarity bit
        WAITPOL OFFSET(9) NUMBITS(1) [],
        /// Burst enable bit
        BURSTEN OFFSET(8) NUMBITS(1) [],
        /// Flash access enable
        /// Enables NOR Flash memory access operations
        FACCEN OFFSET(6) NUMBITS(1) [],
        /// Memory data bus_width width
        MWID OFFSET(4) NUMBITS(2) [
            BITS_8 = 0b00,
            BITS_16 = 0b01
        ],
        /// Memory type
        MTYP OFFSET(2) NUMBITS(2) [
            SRAM = 0b00,
            PSRAM = 0b01,
            NOR = 0b10
        ],
        /// Address/data multiplexing enable bit
        MUXEN OFFSET(1) NUMBITS(1) [],
        /// Memory bank enable bit
        MBKEN OFFSET(0) NUMBITS(1) []
    ],
    BTR [
        /// Access mode
        ACCMOD OFFSET(28) NUMBITS(2) [
            A = 0b00,
            B = 0b01,
            C = 0b10,
            D = 0b11
        ],
        /// Data latency for synchronous memory
        DATLAT OFFSET(24) NUMBITS(4) [],
        /// Clock divide ratio (for FSMC_CLK signal)
        CLKDIV OFFSET(20) NUMBITS(4) [],
        /// Bus turnaround phase duration
        BUSTURN OFFSET(16) NUMBITS(4) [],
        /// Data-phase duration
        DATAST OFFSET(8) NUMBITS(8) [],
        /// Address-hold phase duration
        ADDHLD OFFSET(4) NUMBITS(4) [],
        /// Address setup phase duration
        ADDSET OFFSET(0) NUMBITS(4) []
    ],
    BWTR [
        /// Access mode
        ACCMOD OFFSET(28) NUMBITS(2) [
            A = 0b00,
            B = 0b01,
            C = 0b10,
            D = 0b11
        ],
        BUSTURN OFFSET(16) NUMBITS(4) [],
        /// Data-phase duration
        DATAST OFFSET(8) NUMBITS(8) [],
        /// Address-hold phase duration
        ADDHLD OFFSET(4) NUMBITS(4) [],
        /// Address setup phase duration
        ADDSET OFFSET(0) NUMBITS(4) []
    ]
];

const FSMC_BASE: StaticRef<FsmcBankRegisters> =
    unsafe { StaticRef::new(0xA000_0000 as *const FsmcBankRegisters) };

/// FSMC Bank
#[repr(C)]
pub struct FsmcBank {
    /// Address
    reg: ReadWrite<u16>,
    /// Data
    ram: ReadWrite<u16>,
}

#[repr(usize)]
pub enum FsmcBanks {
    Bank1 = 0,
    Bank2 = 1,
    Bank3 = 2,
    Bank4 = 3,
}

pub const FSMC_BANK1: StaticRef<FsmcBank> =
    unsafe { StaticRef::new(0x60000000 as *const FsmcBank) };
// const FSMC_BANK2_RESERVED: StaticRef<FsmcBank> = unsafe { StaticRef::new(0x0 as *const FsmcBank) };
pub const FSMC_BANK3: StaticRef<FsmcBank> =
    unsafe { StaticRef::new(0x68000000 as *const FsmcBank) };
// const FSMC_BANK4_RESERVED: StaticRef<FsmcBank> = unsafe { StaticRef::new(0x0 as *const FsmcBank) };

pub struct Fsmc<'a> {
    registers: StaticRef<FsmcBankRegisters>,
    bank: [Option<StaticRef<FsmcBank>>; 4],
    clock: FsmcClock<'a>,

    client: OptionalCell<&'static dyn Client>,

    buffer: TakeCell<'static, [u8]>,
    bus_width: Cell<usize>,
    len: Cell<usize>,

    deferred_call: DeferredCall,
}

impl<'a> Fsmc<'a> {
    pub fn new(bank_addr: [Option<StaticRef<FsmcBank>>; 4], clocks: &'a dyn Stm32f4Clocks) -> Self {
        Self {
            registers: FSMC_BASE,
            bank: bank_addr,
            clock: FsmcClock(phclk::PeripheralClock::new(
                phclk::PeripheralClockType::AHB3(phclk::HCLK3::FMC),
                clocks,
            )),
            client: OptionalCell::empty(),

            buffer: TakeCell::empty(),
            bus_width: Cell::new(1),
            len: Cell::new(0),

            deferred_call: DeferredCall::new(),
        }
    }

    pub fn enable(&self) {
        self.registers.bcr1.modify(
            BCR::MBKEN::SET
                + BCR::MUXEN::CLEAR
                + BCR::MTYP::SRAM
                + BCR::MWID::BITS_16
                + BCR::BURSTEN::CLEAR
                + BCR::WAITPOL::CLEAR
                + BCR::WAITCFG::CLEAR
                + BCR::WREN::SET
                + BCR::WAITEN::CLEAR
                + BCR::EXTMOD::SET
                + BCR::ASYNCWAIT::CLEAR
                + BCR::CBURSTRW::CLEAR
                + BCR::WFDIS::SET
                + BCR::CPSIZE::NO_BURST
                + BCR::CCLKEN::CLEAR,
        );
        self.registers.btr1.modify(
            BTR::ADDSET.val(9)
                + BTR::ADDHLD.val(1)
                + BTR::DATAST.val(36)
                + BTR::BUSTURN.val(1)
                + BTR::CLKDIV.val(2)
                + BTR::DATLAT.val(2)
                + BTR::ACCMOD::A,
        );
        self.registers.bwtr1.modify(
            BWTR::ADDSET.val(1)
                + BWTR::ADDHLD.val(1)
                + BWTR::DATAST.val(7)
                + BWTR::BUSTURN.val(0)
                + BWTR::ACCMOD::A,
        );
        self.enable_clock();
    }

    pub fn disable(&self) {
        self.disable_clock();
    }

    pub fn enable_clock(&self) {
        self.clock.enable();
    }

    pub fn disable_clock(&self) {
        self.clock.disable();
    }

    #[inline]
    pub fn read_reg(&self, bank: FsmcBanks) -> Option<u16> {
        self.bank[bank as usize].map_or(None, |bank| Some(bank.ram.get()))
    }

    #[cfg(all(target_arch = "arm", target_os = "none"))]
    #[inline]
    fn write_reg(&self, bank: FsmcBanks, addr: u16) {
        use kernel::utilities::registers::interfaces::Writeable;
        self.bank[bank as usize].map(|bank| bank.reg.set(addr));
        unsafe {
            use core::arch::asm;
            asm!("dsb 0xf");
        }
    }

    #[cfg(all(target_arch = "arm", target_os = "none"))]
    #[inline]
    fn write_data(&self, bank: FsmcBanks, data: u16) {
        use kernel::utilities::registers::interfaces::Writeable;
        self.bank[bank as usize].map(|bank| bank.ram.set(data));
        unsafe {
            use core::arch::asm;
            asm!("dsb 0xf");
        }
    }

    #[cfg(not(all(target_arch = "arm", target_os = "none")))]
    fn write_reg(&self, _bank: FsmcBanks, _addr: u16) {
        unimplemented!()
    }

    #[cfg(not(all(target_arch = "arm", target_os = "none")))]
    fn write_data(&self, _bank: FsmcBanks, _data: u16) {
        unimplemented!()
    }
}

impl DeferredCallClient for Fsmc<'_> {
    fn register(&'static self) {
        self.deferred_call.register(self);
    }

    fn handle_deferred_call(&self) {
        self.buffer.take().map_or_else(
            || {
                self.client.map(move |client| {
                    client.command_complete(None, 0, Ok(()));
                });
            },
            |buffer| {
                self.client.map(move |client| {
                    client.command_complete(Some(buffer), self.len.get(), Ok(()));
                });
            },
        );
    }
}

struct FsmcClock<'a>(phclk::PeripheralClock<'a>);

impl ClockInterface for FsmcClock<'_> {
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

impl Bus8080<'static> for Fsmc<'_> {
    fn set_addr(&self, addr_width: BusWidth, addr: usize) -> Result<(), ErrorCode> {
        match addr_width {
            BusWidth::Bits8 => {
                self.write_reg(FsmcBanks::Bank1, addr as u16);
                self.deferred_call.set();
                Ok(())
            }
            _ => Err(ErrorCode::NOSUPPORT),
        }
    }

    fn write(
        &self,
        data_width: BusWidth,
        buffer: &'static mut [u8],
        len: usize,
    ) -> Result<(), (ErrorCode, &'static mut [u8])> {
        let bytes = data_width.width_in_bytes();
        if buffer.len() >= len * bytes {
            for pos in 0..len {
                let mut data: u16 = 0;
                for byte in 0..bytes {
                    data |= (buffer[bytes * pos
                        + match data_width {
                            BusWidth::Bits8 | BusWidth::Bits16LE => byte,
                            BusWidth::Bits16BE => bytes - byte - 1,
                        }] as u16)
                        << (8 * byte);
                }
                self.write_data(FsmcBanks::Bank1, data);
            }
            self.buffer.replace(buffer);
            self.bus_width.set(bytes);
            self.len.set(len);
            self.deferred_call.set();
            Ok(())
        } else {
            Err((ErrorCode::NOMEM, buffer))
        }
    }

    fn read(
        &self,
        data_width: BusWidth,
        buffer: &'static mut [u8],
        len: usize,
    ) -> Result<(), (ErrorCode, &'static mut [u8])> {
        let bytes = data_width.width_in_bytes();
        if buffer.len() >= len * bytes {
            for pos in 0..len {
                if let Some(data) = self.read_reg(FsmcBanks::Bank1) {
                    for byte in 0..bytes {
                        buffer[bytes * pos
                            + match data_width {
                                BusWidth::Bits8 | BusWidth::Bits16LE => byte,
                                BusWidth::Bits16BE => bytes - byte - 1,
                            }] = (data >> (8 * byte)) as u8;
                    }
                } else {
                    return Err((ErrorCode::NOMEM, buffer));
                }
            }
            self.buffer.replace(buffer);
            self.bus_width.set(bytes);
            self.len.set(len);
            self.deferred_call.set();
            Ok(())
        } else {
            Err((ErrorCode::NOMEM, buffer))
        }
    }

    fn set_client(&self, client: &'static dyn Client) {
        self.client.replace(client);
    }
}
