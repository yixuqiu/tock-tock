// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! SyscallDriver for the HD44780 LCD screen.
//!
//! The LCD must be connected as shown here, because the pins of the LCD are
//! already defined in the kernel, and modifying them means re-compiling the
//! kernel with the modifications.
//!
//! This capsule takes an alarm, an array of pins and one buffer initialized
//! to 0.
//!
//! This capsule uses the TextScreen capsule and implements the TextScreen trait,
//! through which it can receive commands (specific driver commands or write
//! commands) and call specific callbacks (write_complete() or command_complete()).
//!
//! According to the HD44780 datasheet, there must be a delay between certain
//! operations on the device. Since there cannot be a delay while running on
//! kernel mode, the alarm is the best way to implement those delays. To
//! remember the state before and after each delay, the program will be a big
//! state-machine that goes through the possible states defined in the
//! LCDStatus enum. Also, after every command completed, a callback will be called
//! to the text_screen capsule, in order for this capsule to be able to receive new
//! commands. If a command is sent while this capsule is busy, it will return a
//! "BUSY" code.

//! Usage
//! -----
//! ```rust,ignore
//! let lcd = components::hd44780::HD44780Component::new(mux_alarm).finalize(
//!     components::hd44780_component_helper!(
//!         stm32f429zi::tim2::Tim2,
//!         // rs pin
//!         gpio_ports.pins[5][13].as_ref().unwrap(),
//!         // en pin
//!         gpio_ports.pins[4][11].as_ref().unwrap(),
//!         // data 4 pin
//!         gpio_ports.pins[5][14].as_ref().unwrap(),
//!         // data 5 pin
//!         gpio_ports.pins[4][13].as_ref().unwrap(),
//!         // data 6 pin
//!         gpio_ports.pins[5][15].as_ref().unwrap(),
//!         // data 7 pin
//!         gpio_ports.pins[6][14].as_ref().unwrap()
//!     )
//! );
//!
//! let text_screen = components::text_screen::TextScreenComponent::new(board_kernel, lcd)
//!                 .finalize(components::screen_buffer_size!(64));
//! ```
//!
//! Author: Teona Severin <teona.severin9@gmail.com>

use core::cell::Cell;
use kernel::hil::gpio;
use kernel::hil::text_screen::{TextScreen, TextScreenClient};
use kernel::hil::time::{self, Alarm, Frequency};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::ErrorCode;

/// commands
static LCD_CLEARDISPLAY: u8 = 0x01;
static LCD_ENTRYMODESET: u8 = 0x04;
static LCD_DISPLAYCONTROL: u8 = 0x08;
static LCD_FUNCTIONSET: u8 = 0x20;
static LCD_SETDDRAMADDR: u8 = 0x80;

/// flags for display entry mode
static LCD_ENTRYLEFT: u8 = 0x02;
static LCD_ENTRYSHIFTDECREMENT: u8 = 0x00;

/// flags for display on/off control
static LCD_DISPLAYON: u8 = 0x04;
static LCD_CURSORON: u8 = 0x02;
static LCD_BLINKON: u8 = 0x01;
static LCD_BLINKOFF: u8 = 0x00;

/// flags for function set
static LCD_8BITMODE: u8 = 0x10;
static LCD_4BITMODE: u8 = 0x00;
static LCD_2LINE: u8 = 0x08;
static LCD_1LINE: u8 = 0x00;
static LCD_5X8DOTS: u8 = 0x00;

pub const BUF_LEN: usize = 4;

/// The states the program can be in.
#[derive(Copy, Clone, PartialEq)]
enum LCDStatus {
    Idle,
    Begin0,
    Begin0_1,
    Begin1,
    Begin1_2,
    Begin2,
    Begin2_3,
    Begin3,
    Begin4,
    Begin5,
    Begin6,
    Begin7,
    Begin8,
    Begin9,
    Begin10,
    Begin11,
    Begin12,
    Printing,
    PulseLow,
    PulseHigh,
    Command,
    Clear,
}

pub struct HD44780<'a, A: Alarm<'a>> {
    rs_pin: &'a dyn gpio::Pin,
    en_pin: &'a dyn gpio::Pin,
    data_4_pin: &'a dyn gpio::Pin,
    data_5_pin: &'a dyn gpio::Pin,
    data_6_pin: &'a dyn gpio::Pin,
    data_7_pin: &'a dyn gpio::Pin,

    width: Cell<u8>,
    height: Cell<u8>,

    display_function: Cell<u8>,
    display_control: Cell<u8>,
    display_mode: Cell<u8>,
    num_lines: Cell<u8>,
    row_offsets: TakeCell<'static, [u8]>,

    alarm: &'a A,

    lcd_status: Cell<LCDStatus>,
    lcd_after_pulse_status: Cell<LCDStatus>,
    lcd_after_command_status: Cell<LCDStatus>,
    lcd_after_delay_status: Cell<LCDStatus>,
    command_to_finish: Cell<u8>,

    begin_done: Cell<bool>,
    initialized: Cell<bool>,

    text_screen_client: OptionalCell<&'a dyn TextScreenClient>,

    done_printing: Cell<bool>,

    write_buffer: TakeCell<'static, [u8]>,
    write_len: Cell<u8>,
    write_buffer_len: Cell<u8>,
    write_offset: Cell<u8>,
}

impl<'a, A: Alarm<'a>> HD44780<'a, A> {
    pub fn new(
        rs_pin: &'a dyn gpio::Pin,
        en_pin: &'a dyn gpio::Pin,
        data_4_pin: &'a dyn gpio::Pin,
        data_5_pin: &'a dyn gpio::Pin,
        data_6_pin: &'a dyn gpio::Pin,
        data_7_pin: &'a dyn gpio::Pin,
        row_offsets: &'static mut [u8],
        alarm: &'a A,
        width: u8,
        height: u8,
    ) -> HD44780<'a, A> {
        rs_pin.make_output();
        en_pin.make_output();
        data_4_pin.make_output();
        data_5_pin.make_output();
        data_6_pin.make_output();
        data_7_pin.make_output();
        let hd44780 = HD44780 {
            rs_pin: rs_pin,
            en_pin: en_pin,
            data_4_pin: data_4_pin,
            data_5_pin: data_5_pin,
            data_6_pin: data_6_pin,
            data_7_pin: data_7_pin,
            width: Cell::new(width),
            height: Cell::new(height),
            display_function: Cell::new(LCD_4BITMODE | LCD_1LINE | LCD_5X8DOTS),
            display_control: Cell::new(0),
            display_mode: Cell::new(0),
            num_lines: Cell::new(0),
            row_offsets: TakeCell::new(row_offsets),
            alarm: alarm,
            lcd_status: Cell::new(LCDStatus::Idle),
            lcd_after_pulse_status: Cell::new(LCDStatus::Idle),
            lcd_after_command_status: Cell::new(LCDStatus::Idle),
            lcd_after_delay_status: Cell::new(LCDStatus::Idle),
            command_to_finish: Cell::new(0),
            begin_done: Cell::new(false),
            initialized: Cell::new(false),
            text_screen_client: OptionalCell::empty(),
            done_printing: Cell::new(false),
            write_buffer: TakeCell::empty(),
            write_len: Cell::new(0),
            write_buffer_len: Cell::new(0),
            write_offset: Cell::new(0),
        };
        hd44780.init(width, height);

        hd44780
    }

    /// `init()` initializes the functioning parameters and communication
    /// parameters of the LCD, according to its datasheet (HD44780).
    ///
    /// When the init is done, the screen capsule will receive a "screen_is_ready()"
    /// callback, in order to be able to receive other commands.
    ///
    /// `init()` is called after the capsule is instantiated:
    /// - hd44780.init(16,2);
    ///
    fn init(&self, col: u8, row: u8) {
        self.begin_done.set(false);
        self.width.set(col);
        self.height.set(row);

        if row > 1 {
            self.display_function
                .replace(self.display_function.get() | LCD_2LINE);
        }

        self.num_lines.replace(row);
        let _ = self.set_rows(0x00, 0x40, 0x00 + col, 0x40 + col);
    }

    pub fn screen_command(&self, command: usize, op: usize, value: u8) -> Result<(), ErrorCode> {
        if self.lcd_status.get() == LCDStatus::Idle {
            match command {
                1 => {
                    if op == 0 {
                        self.display_control.set(self.display_control.get() | value);
                    } else {
                        self.display_control
                            .set(self.display_control.get() & !value);
                    }
                    self.command_to_finish
                        .replace(LCD_DISPLAYCONTROL | self.display_control.get());
                    self.lcd_command(self.command_to_finish.get(), LCDStatus::Idle);
                    Ok(())
                }

                2 => {
                    self.lcd_clear(LCDStatus::Idle);
                    Ok(())
                }

                _ => Err(ErrorCode::INVAL),
            }
        } else {
            Err(ErrorCode::BUSY)
        }
    }

    /// `set_rows()` sets initializing parameters for the communication.
    ///
    /// Example:
    ///  self.set_rows(0x00, 0x40, 0x00+col, 0x40+col);
    ///
    fn set_rows(&self, row0: u8, row1: u8, row2: u8, row3: u8) -> Result<(), ErrorCode> {
        self.row_offsets.map(|buffer| {
            buffer[0] = row0;
            buffer[1] = row1;
            buffer[2] = row2;
            buffer[3] = row3;
        });
        Ok(())
    }

    /// `pulse()` function starts executing the toggle needed by the device after
    /// each write operation, according to the HD44780 datasheet, figure 26,
    /// toggle that will be continued in the fired() function.
    ///
    /// As argument, there is:
    ///  - the status of the program after the process of pulse is done
    ///
    /// Example:
    ///  self.pulse(LCDStatus::Idle);
    ///
    fn pulse(&self, after_pulse_status: LCDStatus) {
        self.lcd_after_pulse_status.set(after_pulse_status);
        self.en_pin.clear();
        self.set_delay(500, LCDStatus::PulseLow);
    }

    /// `write_4_bits()` will either set or clear each data_pin according to the
    /// value to be written on the device.
    ///
    /// As arguments, there are:
    ///  - the value to be written
    ///  - the next status of the program after writing the value
    ///
    /// Example:
    ///  self.write_4_bits(27, LCDStatus::Idle);
    ///
    fn write_4_bits(&self, value: u8, next_status: LCDStatus) {
        if (value >> 0) & 0x01 != 0 {
            self.data_4_pin.set();
        } else {
            self.data_4_pin.clear();
        }

        if (value >> 1) & 0x01 != 0 {
            self.data_5_pin.set();
        } else {
            self.data_5_pin.clear();
        }

        if (value >> 2) & 0x01 != 0 {
            self.data_6_pin.set();
        } else {
            self.data_6_pin.clear();
        }

        if (value >> 3) & 0x01 != 0 {
            self.data_7_pin.set();
        } else {
            self.data_7_pin.clear();
        }

        self.pulse(next_status);
    }

    /// `continue_ops()` is called after an alarm is fired and continues to
    /// execute the command from the state it was left in before the alarm
    fn continue_ops(&self) {
        let state = self.lcd_status.get();

        match state {
            // the execution of a command was just finished and a callback to the
            // screen capsule will be sent (according to the command type)
            LCDStatus::Idle => {
                self.text_screen_client.map(|client| {
                    if self.begin_done.get() {
                        self.begin_done.set(false);
                        self.initialized.set(true);
                        client.command_complete(Ok(()));
                    } else if self.write_len.get() > 0 {
                        self.write_character();
                    } else if self.done_printing.get() {
                        self.done_printing.set(false);
                        if self.write_buffer.is_some() {
                            self.write_buffer.take().map(|buffer| {
                                client.write_complete(
                                    buffer,
                                    self.write_buffer_len.get() as usize,
                                    Ok(()),
                                )
                            });
                        }
                    } else {
                        client.command_complete(Ok(()));
                    }
                });
            }

            LCDStatus::Begin0 => {
                self.rs_pin.clear();
                self.en_pin.clear();

                if (self.display_function.get() & LCD_8BITMODE) == 0 {
                    self.write_4_bits(0x03, LCDStatus::Begin0_1);
                } else {
                    self.rs_pin.clear();
                    self.lcd_command(
                        (LCD_FUNCTIONSET | self.display_function.get()) >> 4,
                        LCDStatus::Begin4,
                    );
                }
            }

            LCDStatus::Begin0_1 => {
                self.set_delay(200, LCDStatus::Begin1);
            }

            LCDStatus::Begin1 => {
                self.write_4_bits(0x03, LCDStatus::Begin1_2);
            }

            LCDStatus::Begin1_2 => {
                self.set_delay(200, LCDStatus::Begin2);
            }

            LCDStatus::Begin2 => {
                self.write_4_bits(0x03, LCDStatus::Begin2_3);
            }

            LCDStatus::Begin2_3 => {
                self.set_delay(500, LCDStatus::Begin3);
            }

            LCDStatus::Begin3 => {
                self.write_4_bits(0x02, LCDStatus::Begin9);
            }

            LCDStatus::Begin4 => {
                self.command_to_finish
                    .set(LCD_FUNCTIONSET | self.display_function.get());
                self.lcd_command(
                    LCD_FUNCTIONSET | self.display_function.get(),
                    LCDStatus::Begin5,
                );
            }

            LCDStatus::Begin5 => self.set_delay(200, LCDStatus::Begin6),

            LCDStatus::Begin6 => {
                self.lcd_command(
                    LCD_FUNCTIONSET | self.display_function.get(),
                    LCDStatus::Begin7,
                );
            }

            LCDStatus::Begin7 => {
                self.set_delay(500, LCDStatus::Begin8);
            }

            LCDStatus::Begin8 => {
                self.lcd_command(
                    LCD_FUNCTIONSET | self.display_function.get(),
                    LCDStatus::Begin9,
                );
            }

            LCDStatus::Begin9 => {
                self.command_to_finish
                    .set(LCD_FUNCTIONSET | self.display_function.get());
                self.lcd_command(
                    LCD_FUNCTIONSET | self.display_function.get(),
                    LCDStatus::Begin10,
                );
            }

            LCDStatus::Begin10 => {
                self.display_control
                    .set(LCD_DISPLAYON | LCD_CURSORON | LCD_BLINKOFF);
                self.lcd_display(LCDStatus::Begin11);
            }

            LCDStatus::Begin11 => {
                self.lcd_clear(LCDStatus::Begin12);
            }

            LCDStatus::Begin12 => {
                self.display_mode
                    .set(LCD_ENTRYLEFT | LCD_ENTRYSHIFTDECREMENT);
                self.command_to_finish
                    .set(LCD_ENTRYMODESET | self.display_mode.get());
                self.begin_done.set(true);
                self.lcd_command(self.command_to_finish.get(), LCDStatus::Idle);
            }

            LCDStatus::Clear => {
                self.set_delay(500, self.lcd_after_delay_status.get());
            }

            LCDStatus::Printing => {
                self.write_4_bits(self.command_to_finish.get(), LCDStatus::Idle);
            }

            LCDStatus::PulseLow => {
                self.en_pin.set();
                self.set_delay(500, LCDStatus::PulseHigh);
            }

            LCDStatus::Command => {
                self.write_4_bits(
                    self.command_to_finish.get(),
                    self.lcd_after_command_status.get(),
                );
            }

            LCDStatus::PulseHigh => {
                self.en_pin.clear();
                self.set_delay(500, self.lcd_after_pulse_status.get());
            }
        }
    }

    /// `lcd_display()` will call lcd_command with certain arguments for the display
    /// initialization.
    ///
    /// As argument, there is:
    ///  - the status of the program after setting the display
    ///
    /// Example:
    ///  self.lcd_display(LCDStatus::Idle);
    ///
    fn lcd_display(&self, next_state: LCDStatus) {
        self.command_to_finish
            .set(LCD_DISPLAYCONTROL | self.display_control.get());
        self.lcd_command(LCD_DISPLAYCONTROL | self.display_control.get(), next_state);
    }

    /// `lcd_command()` is the main function that communicates with the device, and
    /// sends certain values received as arguments to the device (through
    /// write_4_bits function). Due to the delays, the function is continued in
    /// the fired() function.
    ///
    /// As arguments, there are:
    ///  - the value to be sent to the device
    ///  - the next status of the program after sending the value
    ///
    /// Example:
    ///  self.lcd_command(LCD_CLEARDISPLAY, LCDStatus::Clear);
    ///
    fn lcd_command(&self, value: u8, next_state: LCDStatus) {
        self.lcd_after_command_status.set(next_state);
        self.command_to_finish.set(value);
        self.rs_pin.clear();
        self.write_4_bits(value >> 4, LCDStatus::Command);
    }

    /// `lcd_clear()` clears the lcd and brings the cursor at position (0,0).
    ///
    /// As argument, there is:
    ///  - the status of the program after clearing the display
    ///
    /// Example:
    ///  self.clear(LCDStatus::Idle);
    ///
    fn lcd_clear(&self, next_state: LCDStatus) {
        self.lcd_after_delay_status.set(next_state);
        self.lcd_command(LCD_CLEARDISPLAY, LCDStatus::Clear);
    }

    /// `set_delay()` sets an alarm and saved the next state after that.
    ///
    /// As argument, there are:
    ///  - the duration of the alarm:
    ///      - 10 means 100 ms
    ///      - 100 means 10 ms
    ///      - 500 means 2 ms
    ///  - the status of the program after the alarm fires
    ///
    /// Example:
    ///  self.set_delay(10, LCDStatus::Idle);
    ///
    fn set_delay(&self, timer: u32, next_status: LCDStatus) {
        self.lcd_status.set(next_status);
        self.alarm.set_alarm(
            self.alarm.now(),
            A::Ticks::from(<A::Frequency>::frequency() / timer),
        );
    }

    /// `write_character()` will send the next character to be written on the
    /// LCD display. The character is saved in the "write_buffer" buffer.
    ///
    /// Example:
    /// - self.write_character();
    ///
    fn write_character(&self) {
        let offset = self.write_offset.get() as usize;
        let mut value = 0;
        self.write_buffer.map(|buffer| {
            value = buffer[offset];
        });
        self.done_printing.set(false);
        self.write_offset.set(self.write_offset.get() + 1);
        self.write_len.set(self.write_len.get() - 1);
        if self.write_len.get() == 0 {
            self.done_printing.set(true);
        }
        self.rs_pin.set();
        self.command_to_finish.set(value);
        self.write_4_bits(value >> 4, LCDStatus::Printing);
    }

    /// `set_cursor()` sends a command to the LCD display about the position for
    /// the cursor to be set to.
    ///
    /// As argument, there are:
    /// - the column for the position
    /// - the row for the position
    ///
    /// Example:
    /// - self.set_cursor(16,2);
    ///
    fn set_cursor(&self, col: u8, row: u8) {
        let mut value: u8 = 0;
        self.row_offsets.map(|buffer| {
            value = buffer[row as usize];
        });
        self.command_to_finish
            .replace(LCD_SETDDRAMADDR | (col + value));
        self.lcd_command(self.command_to_finish.get(), LCDStatus::Idle);
    }
}

impl<'a, A: Alarm<'a>> time::AlarmClient for HD44780<'a, A> {
    /// `alarm()` is called after each alarm finished, and depending on the
    /// current state of the program, the next step in being decided.
    fn alarm(&self) {
        self.continue_ops();
    }
}

impl<'a, A: Alarm<'a>> TextScreen<'a> for HD44780<'a, A> {
    fn get_size(&self) -> (usize, usize) {
        (16, 2)
    }

    fn print(
        &self,
        buffer: &'static mut [u8],
        len: usize,
    ) -> Result<(), (ErrorCode, &'static mut [u8])> {
        if self.lcd_status.get() == LCDStatus::Idle {
            self.write_buffer.replace(buffer);
            self.write_len.replace(len as u8);
            self.write_buffer_len.replace(len as u8);
            self.write_offset.set(0);
            self.write_character();
            Ok(())
        } else {
            Err((ErrorCode::BUSY, buffer))
        }
    }

    fn set_cursor(&self, x_position: usize, y_position: usize) -> Result<(), ErrorCode> {
        if self.lcd_status.get() == LCDStatus::Idle {
            let mut line_number: u8 = y_position as u8;
            if line_number >= 4 {
                line_number = 3;
            }

            if line_number >= self.num_lines.get() {
                line_number = self.num_lines.get() - 1;
            }

            self.set_cursor(x_position as u8, line_number);
            Ok(())
        } else {
            Err(ErrorCode::BUSY)
        }
    }

    fn hide_cursor(&self) -> Result<(), ErrorCode> {
        self.screen_command(1, 1, LCD_CURSORON)
    }

    fn show_cursor(&self) -> Result<(), ErrorCode> {
        self.screen_command(1, 0, LCD_CURSORON)
    }

    fn blink_cursor_on(&self) -> Result<(), ErrorCode> {
        self.screen_command(1, 0, LCD_BLINKON)
    }

    fn blink_cursor_off(&self) -> Result<(), ErrorCode> {
        self.screen_command(1, 1, LCD_BLINKON)
    }

    fn display_on(&self) -> Result<(), ErrorCode> {
        if !self.initialized.get() {
            if self.lcd_status.get() == LCDStatus::Idle {
                self.set_delay(10, LCDStatus::Begin0);
                Ok(())
            } else {
                Err(ErrorCode::BUSY)
            }
        } else {
            self.screen_command(1, 0, LCD_DISPLAYON)
        }
    }

    fn display_off(&self) -> Result<(), ErrorCode> {
        self.screen_command(1, 1, LCD_DISPLAYON)
    }

    fn clear(&self) -> Result<(), ErrorCode> {
        self.screen_command(2, 0, 0)
    }

    fn set_client(&self, client: Option<&'a dyn TextScreenClient>) {
        if let Some(client) = client {
            self.text_screen_client.set(client);
        } else {
            self.text_screen_client.clear();
        }
    }
}
