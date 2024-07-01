// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the Silicon Labs SI7021 temperature/humidity sensor.
//!
//! <https://www.silabs.com/products/sensors/humidity-sensors/Pages/si7013-20-21.aspx>
//!
//! > The Si7006/13/20/21/34 devices are Silicon Labs’ latest generation I2C
//! > relative humidity and temperature sensors. All members of this device
//! > family combine fully factory-calibrated humidity and temperature sensor
//! > elements with an analog to digital converter, signal processing and an I2C
//! > host interface. Patented use of industry-standard low-K polymer
//! > dielectrics provides excellent accuracy and long term stability, along
//! > with low drift and low hysteresis. The innovative CMOS design also offers
//! > the lowest power consumption in the industry for a relative humidity and
//! > temperature sensor. The Si7013/20/21/34 devices are designed for high-
//! > accuracy applications, while the Si7006 is targeted toward lower-accuracy
//! > applications that traditionally have used discrete RH/T sensors.
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! # use kernel::static_init;
//! # use capsules::virtual_alarm::VirtualMuxAlarm;
//!
//! let si7021_i2c = static_init!(
//!     capsules::virtual_i2c::I2CDevice,
//!     capsules::virtual_i2c::I2CDevice::new(i2c_bus, 0x40));
//! let si7021_virtual_alarm = static_init!(
//!     VirtualMuxAlarm<'static, sam4l::ast::Ast>,
//!     VirtualMuxAlarm::new(mux_alarm));
//! si7021_virtual_alarm.setup();
//!
//! let si7021 = static_init!(
//!     capsules::si7021::SI7021<'static, VirtualMuxAlarm<'static, sam4l::ast::Ast>>,
//!     capsules::si7021::SI7021::new(si7021_i2c,
//!         si7021_virtual_alarm,
//!         &mut capsules::si7021::BUFFER));
//! si7021_i2c.set_client(si7021);
//! si7021_virtual_alarm.set_client(si7021);
//! ```

use core::cell::Cell;
use kernel::hil::i2c;
use kernel::hil::time::{self, ConvertTicks};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::ErrorCode;

#[allow(dead_code)]
enum Registers {
    MeasRelativeHumidityHoldMode = 0xe5,
    MeasRelativeHumidityNoHoldMode = 0xf5,
    MeasTemperatureHoldMode = 0xe3,
    MeasTemperatureNoHoldMode = 0xf3,
    ReadTemperaturePreviousRHMeasurement = 0xe0,
    Reset = 0xfe,
    WriteRHTUserRegister1 = 0xe6,
    ReadRHTUserRegister1 = 0xe7,
    WriteHeaterControlRegister = 0x51,
    ReadHeaterControlRegister = 0x11,
    ReadElectronicIdByteOneA = 0xfa,
    ReadElectronicIdByteOneB = 0x0f,
    ReadElectronicIdByteTwoA = 0xfc,
    ReadElectronicIdByteTwoB = 0xc9,
    ReadFirmwareVersionA = 0x84,
    ReadFirmwareVersionB = 0xb8,
}

/// States of the I2C protocol with the LPS331AP.
#[derive(Clone, Copy, PartialEq)]
enum State {
    Idle,
    WaitTemp,
    WaitRh,

    /// States to read the internal ID
    SelectElectronicId1,
    ReadElectronicId1,
    SelectElectronicId2,
    ReadElectronicId2,

    /// States to take the current measurement
    TakeTempMeasurementInit,
    TakeRhMeasurementInit,
    ReadRhMeasurement,
    ReadTempMeasurement,
    GotTempMeasurement,
    GotRhMeasurement,
}

#[derive(PartialEq, Eq, Copy, Clone)]
enum OnDeck {
    Nothing,
    Temperature,
    Humidity,
}

pub struct SI7021<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> {
    i2c: &'a I,
    alarm: &'a A,
    temp_callback: OptionalCell<&'a dyn kernel::hil::sensors::TemperatureClient>,
    humidity_callback: OptionalCell<&'a dyn kernel::hil::sensors::HumidityClient>,
    state: Cell<State>,
    on_deck: Cell<OnDeck>,
    buffer: TakeCell<'static, [u8]>,
}

impl<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> SI7021<'a, A, I> {
    pub fn new(i2c: &'a I, alarm: &'a A, buffer: &'static mut [u8]) -> SI7021<'a, A, I> {
        // setup and return struct
        SI7021 {
            i2c: i2c,
            alarm: alarm,
            temp_callback: OptionalCell::empty(),
            humidity_callback: OptionalCell::empty(),
            state: Cell::new(State::Idle),
            on_deck: Cell::new(OnDeck::Nothing),
            buffer: TakeCell::new(buffer),
        }
    }

    pub fn read_id(&self) {
        self.buffer.take().map(|buffer| {
            // turn on i2c to send commands
            self.i2c.enable();

            buffer[0] = Registers::ReadElectronicIdByteOneA as u8;
            buffer[1] = Registers::ReadElectronicIdByteOneB as u8;
            // TODO verify errors
            let _ = self.i2c.write(buffer, 2);
            self.state.set(State::SelectElectronicId1);
        });
    }

    fn init_measurement(&self, buffer: &'static mut [u8]) {
        let delay = self.alarm.ticks_from_ms(20);
        self.alarm.set_alarm(self.alarm.now(), delay);

        // Now wait for timer to expire
        self.buffer.replace(buffer);
        self.i2c.disable();
    }

    fn set_idle(&self, buffer: &'static mut [u8]) {
        self.buffer.replace(buffer);
        self.i2c.disable();
        self.state.set(State::Idle);
    }
}

impl<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> i2c::I2CClient for SI7021<'a, A, I> {
    fn command_complete(&self, buffer: &'static mut [u8], _status: Result<(), i2c::Error>) {
        match self.state.get() {
            State::SelectElectronicId1 => {
                // TODO verify errors
                let _ = self.i2c.read(buffer, 8);
                self.state.set(State::ReadElectronicId1);
            }
            State::ReadElectronicId1 => {
                buffer[6] = buffer[0];
                buffer[7] = buffer[1];
                buffer[8] = buffer[2];
                buffer[9] = buffer[3];
                buffer[10] = buffer[4];
                buffer[11] = buffer[5];
                buffer[12] = buffer[6];
                buffer[13] = buffer[7];
                buffer[0] = Registers::ReadElectronicIdByteTwoA as u8;
                buffer[1] = Registers::ReadElectronicIdByteTwoB as u8;
                // TODO verify errors
                let _ = self.i2c.write(buffer, 2);
                self.state.set(State::SelectElectronicId2);
            }
            State::SelectElectronicId2 => {
                // TODO verify errors
                let _ = self.i2c.read(buffer, 6);
                self.state.set(State::ReadElectronicId2);
            }
            State::ReadElectronicId2 => {
                self.set_idle(buffer);
            }
            State::TakeTempMeasurementInit => {
                self.init_measurement(buffer);
                self.state.set(State::WaitTemp);
            }
            State::TakeRhMeasurementInit => {
                self.init_measurement(buffer);
                self.state.set(State::WaitRh);
            }
            State::ReadRhMeasurement => {
                // TODO verify errors
                let _ = self.i2c.read(buffer, 2);
                self.state.set(State::GotRhMeasurement);
            }
            State::ReadTempMeasurement => {
                // TODO verify errors
                let _ = self.i2c.read(buffer, 2);
                self.state.set(State::GotTempMeasurement);
            }
            State::GotTempMeasurement => {
                // Temperature in hundredths of degrees centigrade
                let temp_raw = ((buffer[0] as u32) << 8) | (buffer[1] as u32);
                let temp = ((temp_raw * 17572) / 65536) as i32 - 4685;

                self.temp_callback.map(|cb| cb.callback(Ok(temp)));

                match self.on_deck.get() {
                    OnDeck::Humidity => {
                        self.on_deck.set(OnDeck::Nothing);
                        buffer[0] = Registers::MeasRelativeHumidityNoHoldMode as u8;
                        // TODO verify errors
                        let _ = self.i2c.write(buffer, 1);
                        self.state.set(State::TakeRhMeasurementInit);
                    }
                    _ => {
                        self.set_idle(buffer);
                    }
                }
            }
            State::GotRhMeasurement => {
                // Humidity in hundredths of percent
                let humidity_raw = ((buffer[0] as u32) << 8) | (buffer[1] as u32);
                let humidity = (((humidity_raw * 125 * 100) / 65536) - 600) as u16;

                self.humidity_callback
                    .map(|cb| cb.callback(humidity as usize));
                match self.on_deck.get() {
                    OnDeck::Temperature => {
                        self.on_deck.set(OnDeck::Nothing);
                        buffer[0] = Registers::MeasTemperatureNoHoldMode as u8;
                        // TODO verify errors
                        let _ = self.i2c.write(buffer, 1);
                        self.state.set(State::TakeTempMeasurementInit);
                    }
                    _ => {
                        self.set_idle(buffer);
                    }
                }
            }
            _ => {}
        }
    }
}

impl<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> kernel::hil::sensors::TemperatureDriver<'a>
    for SI7021<'a, A, I>
{
    fn read_temperature(&self) -> Result<(), ErrorCode> {
        // This chip handles both humidity and temperature measurements. We can
        // only start a new measurement if the chip is idle. If it isn't then we
        // can put this request "on deck" and it will happen after the
        // temperature measurement has finished.
        if self.state.get() == State::Idle {
            self.buffer.take().map_or(Err(ErrorCode::BUSY), |buffer| {
                // turn on i2c to send commands
                self.i2c.enable();

                buffer[0] = Registers::MeasTemperatureNoHoldMode as u8;
                // TODO verify errors
                let _ = self.i2c.write(buffer, 1);
                self.state.set(State::TakeTempMeasurementInit);
                Ok(())
            })
        } else {
            // Queue this request if nothing else queued.
            if self.on_deck.get() == OnDeck::Nothing {
                self.on_deck.set(OnDeck::Temperature);
                Ok(())
            } else {
                Err(ErrorCode::BUSY)
            }
        }
    }

    fn set_client(&self, client: &'a dyn kernel::hil::sensors::TemperatureClient) {
        self.temp_callback.set(client);
    }
}

impl<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> kernel::hil::sensors::HumidityDriver<'a>
    for SI7021<'a, A, I>
{
    fn read_humidity(&self) -> Result<(), ErrorCode> {
        // This chip handles both humidity and temperature measurements. We can
        // only start a new measurement if the chip is idle. If it isn't then we
        // can put this request "on deck" and it will happen after the
        // temperature measurement has finished.
        if self.state.get() == State::Idle {
            self.buffer.take().map_or(Err(ErrorCode::BUSY), |buffer| {
                // turn on i2c to send commands
                self.i2c.enable();

                buffer[0] = Registers::MeasRelativeHumidityNoHoldMode as u8;
                // TODO verify errors
                let _ = self.i2c.write(buffer, 1);
                self.state.set(State::TakeRhMeasurementInit);
                Ok(())
            })
        } else {
            // Not idle, so queue this request if nothing else is queued. If we have already
            // queued a request return an error.
            if self.on_deck.get() == OnDeck::Nothing {
                self.on_deck.set(OnDeck::Humidity);
                Ok(())
            } else {
                Err(ErrorCode::BUSY)
            }
        }
    }

    fn set_client(&self, client: &'a dyn kernel::hil::sensors::HumidityClient) {
        self.humidity_callback.set(client);
    }
}

impl<'a, A: time::Alarm<'a>, I: i2c::I2CDevice> time::AlarmClient for SI7021<'a, A, I> {
    fn alarm(&self) {
        self.buffer.take().map(|buffer| {
            // turn on i2c to send commands
            self.i2c.enable();

            // TODO verify errors
            let _ = self.i2c.read(buffer, 2);
            match self.state.get() {
                State::WaitRh => self.state.set(State::ReadRhMeasurement),
                State::WaitTemp => self.state.set(State::ReadTempMeasurement),
                _ => (),
            }
        });
    }
}
