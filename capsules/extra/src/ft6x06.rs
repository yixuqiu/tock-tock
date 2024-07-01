// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the FT6x06 Touch Panel.
//!
//! I2C Interface
//!
//! <http://www.tvielectronics.com/ocart/download/controller/FT6206.pdf>
//!
//! Usage
//! -----
//!
//! ```rust,ignore
//! let mux_i2c = components::i2c::I2CMuxComponent::new(&stm32f4xx::i2c::I2C1)
//!     .finalize(components::i2c_mux_component_helper!());
//!
//! let ft6x06 = components::ft6x06::Ft6x06Component::new(
//!     stm32f412g::gpio::PinId::PG05.get_pin().as_ref().unwrap(),
//! )
//! .finalize(components::ft6x06_i2c_component_helper!(mux_i2c));
//!
//! Author: Alexandru Radovici <msg4alex@gmail.com>

#![allow(non_camel_case_types)]

use core::cell::Cell;
use enum_primitive::cast::FromPrimitive;
use enum_primitive::enum_from_primitive;
use kernel::hil::gpio;
use kernel::hil::i2c;
use kernel::hil::touch::{self, GestureEvent, TouchEvent, TouchStatus};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::ErrorCode;

pub static NO_TOUCH: TouchEvent = TouchEvent {
    id: 0,
    x: 0,
    y: 0,
    status: TouchStatus::Released,
    size: None,
    pressure: None,
};

enum_from_primitive! {
    enum Registers {
        REG_GEST_ID = 0x01,
        REG_TD_STATUS = 0x02,
        REG_CHIPID = 0xA3,
    }
}

pub struct Ft6x06<'a, I: i2c::I2CDevice> {
    i2c: &'a I,
    interrupt_pin: &'a dyn gpio::InterruptPin<'a>,
    touch_client: OptionalCell<&'a dyn touch::TouchClient>,
    gesture_client: OptionalCell<&'a dyn touch::GestureClient>,
    multi_touch_client: OptionalCell<&'a dyn touch::MultiTouchClient>,
    num_touches: Cell<usize>,
    buffer: TakeCell<'static, [u8]>,
    events: TakeCell<'static, [TouchEvent]>,
}

impl<'a, I: i2c::I2CDevice> Ft6x06<'a, I> {
    pub fn new(
        i2c: &'a I,
        interrupt_pin: &'a dyn gpio::InterruptPin<'a>,
        buffer: &'static mut [u8],
        events: &'static mut [TouchEvent],
    ) -> Ft6x06<'a, I> {
        // setup and return struct
        interrupt_pin.enable_interrupts(gpio::InterruptEdge::FallingEdge);
        Ft6x06 {
            i2c: i2c,
            interrupt_pin: interrupt_pin,
            touch_client: OptionalCell::empty(),
            gesture_client: OptionalCell::empty(),
            multi_touch_client: OptionalCell::empty(),
            num_touches: Cell::new(0),
            buffer: TakeCell::new(buffer),
            events: TakeCell::new(events),
        }
    }
}

impl<'a, I: i2c::I2CDevice> i2c::I2CClient for Ft6x06<'a, I> {
    fn command_complete(&self, buffer: &'static mut [u8], _status: Result<(), i2c::Error>) {
        self.num_touches.set((buffer[1] & 0x0F) as usize);
        self.touch_client.map(|client| {
            if self.num_touches.get() <= 2 {
                let status = match buffer[2] >> 6 {
                    0x00 => Some(TouchStatus::Pressed),
                    0x01 => Some(TouchStatus::Released),
                    0x02 => Some(TouchStatus::Moved),
                    _ => None,
                };
                if let Some(status) = status {
                    let x = (((buffer[2] & 0x0F) as u16) << 8) + (buffer[3] as u16);
                    let y = (((buffer[4] & 0x0F) as u16) << 8) + (buffer[5] as u16);
                    let pressure = Some(buffer[6] as u16);
                    let size = Some(buffer[7] as u16);
                    client.touch_event(TouchEvent {
                        status,
                        x,
                        y,
                        id: 0,
                        pressure,
                        size,
                    });
                }
            }
        });
        self.gesture_client.map(|client| {
            if self.num_touches.get() <= 2 {
                let gesture_event = match buffer[0] {
                    0x10 => Some(GestureEvent::SwipeUp),
                    0x14 => Some(GestureEvent::SwipeRight),
                    0x18 => Some(GestureEvent::SwipeDown),
                    0x1C => Some(GestureEvent::SwipeLeft),
                    0x48 => Some(GestureEvent::ZoomIn),
                    0x49 => Some(GestureEvent::ZoomOut),
                    _ => None,
                };
                if let Some(gesture) = gesture_event {
                    client.gesture_event(gesture);
                }
            }
        });
        self.multi_touch_client.map(|client| {
            if self.num_touches.get() <= 2 {
                let mut num_touches = 0;
                for touch_event in 0..2 {
                    let status = match buffer[touch_event * 6 + 2] >> 6 {
                        0x00 => Some(TouchStatus::Pressed),
                        0x01 => Some(TouchStatus::Released),
                        0x02 => Some(TouchStatus::Moved),
                        _ => None,
                    };
                    if let Some(status) = status {
                        let x = (((buffer[touch_event * 6 + 2] & 0x0F) as u16) << 8)
                            + (buffer[touch_event * 6 + 3] as u16);
                        let y = (((buffer[touch_event * 6 + 4] & 0x0F) as u16) << 8)
                            + (buffer[touch_event * 6 + 5] as u16);
                        let pressure = Some(buffer[touch_event * 6 + 6] as u16);
                        let size = Some(buffer[touch_event * 6 + 7] as u16);
                        let id = (buffer[touch_event * 6 + 4] >> 4) as usize;
                        self.events.map(|buffer| {
                            buffer[num_touches] = TouchEvent {
                                status,
                                x,
                                y,
                                id,
                                size,
                                pressure,
                            };
                        });
                        num_touches += 1;
                    }
                }
                self.events.map(|buffer| {
                    client.touch_events(buffer, num_touches);
                });
            }
        });
        self.buffer.replace(buffer);
        self.interrupt_pin
            .enable_interrupts(gpio::InterruptEdge::FallingEdge);
    }
}

impl<'a, I: i2c::I2CDevice> gpio::Client for Ft6x06<'a, I> {
    fn fired(&self) {
        self.buffer.take().map(|buffer| {
            self.interrupt_pin.disable_interrupts();

            buffer[0] = Registers::REG_GEST_ID as u8;

            match self.i2c.write_read(buffer, 1, 15) {
                Ok(()) => {}
                Err((_err, buffer)) => {
                    self.buffer.replace(buffer);
                    self.interrupt_pin
                        .enable_interrupts(gpio::InterruptEdge::FallingEdge);
                }
            }
        });
    }
}

impl<'a, I: i2c::I2CDevice> touch::Touch<'a> for Ft6x06<'a, I> {
    fn enable(&self) -> Result<(), ErrorCode> {
        Ok(())
    }

    fn disable(&self) -> Result<(), ErrorCode> {
        Ok(())
    }

    fn set_client(&self, client: &'a dyn touch::TouchClient) {
        self.touch_client.replace(client);
    }
}

impl<'a, I: i2c::I2CDevice> touch::Gesture<'a> for Ft6x06<'a, I> {
    fn set_client(&self, client: &'a dyn touch::GestureClient) {
        self.gesture_client.replace(client);
    }
}

impl<'a, I: i2c::I2CDevice> touch::MultiTouch<'a> for Ft6x06<'a, I> {
    fn enable(&self) -> Result<(), ErrorCode> {
        Ok(())
    }

    fn disable(&self) -> Result<(), ErrorCode> {
        Ok(())
    }

    fn get_num_touches(&self) -> usize {
        2
    }

    fn get_touch(&self, index: usize) -> Option<TouchEvent> {
        self.buffer.map_or(None, |buffer| {
            if index <= self.num_touches.get() {
                // a touch has 7 bytes
                let offset = index * 7;
                let status = match buffer[offset + 1] >> 6 {
                    0x00 => TouchStatus::Pressed,
                    0x01 => TouchStatus::Released,
                    0x02 => TouchStatus::Moved,
                    _ => TouchStatus::Released,
                };
                let x = (((buffer[offset + 2] & 0x0F) as u16) << 8) + (buffer[offset + 3] as u16);
                let y = (((buffer[offset + 4] & 0x0F) as u16) << 8) + (buffer[offset + 5] as u16);
                let pressure = Some(buffer[offset + 6] as u16);
                let size = Some(buffer[offset + 7] as u16);
                Some(TouchEvent {
                    status,
                    x,
                    y,
                    id: 0,
                    pressure,
                    size,
                })
            } else {
                None
            }
        })
    }

    fn set_client(&self, client: &'a dyn touch::MultiTouchClient) {
        self.multi_touch_client.replace(client);
    }
}
