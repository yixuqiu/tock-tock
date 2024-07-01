// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! True random number generator

use crate::clocks::{phclk, Stm32f4Clocks};
use kernel::hil;
use kernel::hil::entropy::Continue;
use kernel::platform::chip::ClockInterface;
use kernel::utilities::cells::OptionalCell;
use kernel::utilities::registers::interfaces::{ReadWriteable, Readable};
use kernel::utilities::registers::{register_bitfields, ReadOnly, ReadWrite};
use kernel::utilities::StaticRef;
use kernel::ErrorCode;

#[repr(C)]
pub struct RngRegisters {
    cr: ReadWrite<u32, Control::Register>,
    sr: ReadWrite<u32, Status::Register>,
    data: ReadOnly<u32, Data::Register>,
}

register_bitfields![u32,
    Control [
        /// Clock error detection
        CED OFFSET(5) NUMBITS(1) [
            ENABLE = 0,
            DISABLE = 1
        ],
        /// Interrupt enable
        IE OFFSET(3) NUMBITS(1) [],
        /// True random number generator enable
        RNGEN OFFSET(2) NUMBITS(1) []
    ],
    Status [
        /// Seed error interrupt status
        SEIS OFFSET(6) NUMBITS(1) [],
        /// Clock error interrupt status
        CEIS OFFSET(5) NUMBITS(1) [],
        /// Seed error current status
        SECS OFFSET(2) NUMBITS(1) [],
        /// Clock error current status
        CECS OFFSET(1) NUMBITS(1) [],
        /// Data ready
        DRDY OFFSET(0) NUMBITS(1) []
    ],
    Data [
        /// Random data
        RNDATA OFFSET(0) NUMBITS(32) []
    ]
];

pub struct Trng<'a> {
    registers: StaticRef<RngRegisters>,
    clock: RngClock<'a>,
    client: OptionalCell<&'a dyn hil::entropy::Client32>,
}

impl<'a> Trng<'a> {
    pub const fn new(
        registers: StaticRef<RngRegisters>,
        clocks: &'a dyn Stm32f4Clocks,
    ) -> Trng<'a> {
        Trng {
            registers: registers,
            clock: RngClock(phclk::PeripheralClock::new(
                phclk::PeripheralClockType::AHB2(phclk::HCLK2::RNG),
                clocks,
            )),
            client: OptionalCell::empty(),
        }
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

    pub fn handle_interrupt(&self) {
        if self.registers.sr.is_set(Status::SEIS) {
            self.registers.sr.modify(Status::SEIS::CLEAR);

            // Throw away the content of the data register.
            self.registers.data.read(Data::RNDATA);

            // Restart the rng.
            self.registers.cr.modify(Control::RNGEN::CLEAR);
            self.registers.cr.modify(Control::RNGEN::SET);
            return;
        } else if self.registers.sr.is_set(Status::CEIS) {
            self.clock.0.configure_rng_clock();
            self.registers.sr.modify(Status::CEIS::CLEAR);
            return;
        }

        self.client.map(|client| {
            let res = client.entropy_available(&mut TrngIter(self), Ok(()));
            if let Continue::Done = res {
                self.registers.cr.modify(Control::IE::CLEAR);
                self.registers.cr.modify(Control::RNGEN::CLEAR);
            }
        });
    }
}

struct RngClock<'a>(phclk::PeripheralClock<'a>);

impl ClockInterface for RngClock<'_> {
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

struct TrngIter<'a, 'b: 'a>(&'a Trng<'b>);

impl Iterator for TrngIter<'_, '_> {
    type Item = u32;

    fn next(&mut self) -> Option<u32> {
        if self.0.registers.sr.is_set(Status::DRDY) {
            // This also clears the DRDY bit in the Status register.
            Some(self.0.registers.data.read(Data::RNDATA))
        } else {
            None
        }
    }
}

impl<'a> hil::entropy::Entropy32<'a> for Trng<'a> {
    fn get(&self) -> Result<(), ErrorCode> {
        // Enable interrupts.
        self.registers.cr.modify(Control::IE::SET);
        self.registers.cr.modify(Control::RNGEN::SET);

        Ok(())
    }

    fn cancel(&self) -> Result<(), ErrorCode> {
        self.registers.cr.modify(Control::RNGEN::CLEAR);
        self.registers.cr.modify(Control::IE::CLEAR);

        Ok(())
    }

    fn set_client(&'a self, client: &'a dyn hil::entropy::Client32) {
        self.client.set(client);
    }
}
