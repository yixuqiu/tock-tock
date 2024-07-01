// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! Provides userspace with access to the text screen.
//!
//! Usage:
//! -----
//!
//! You need a screen that provides the `hil::text_screen::TextScreen`
//! trait.
//!
//! ```rust,ignore
//! let text_screen = components::text_screen::TextScreenComponent::new(board_kernel, lcd)
//!         .finalize(components::screen_buffer_size!(64));
//! ```

use core::cmp;

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil;
use kernel::processbuffer::ReadableProcessBuffer;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::{OptionalCell, TakeCell};
use kernel::{ErrorCode, ProcessId};

/// Syscall driver number.
use capsules_core::driver;
pub const DRIVER_NUM: usize = driver::NUM::TextScreen as usize;

/// Ids for read-only allow buffers
mod ro_allow {
    pub const SHARED: usize = 0;
    /// The number of allow buffers the kernel stores for this grant
    pub const COUNT: u8 = 1;
}

#[derive(Clone, Copy, PartialEq)]
enum TextScreenCommand {
    Idle,
    GetResolution,
    Display,
    NoDisplay,
    Blink,
    NoBlink,
    SetCursor,
    NoCursor,
    ShowCursor,
    Write,
    Clear,
    Home,
}

pub struct App {
    pending_command: bool,
    write_len: usize,
    command: TextScreenCommand,
    data1: usize,
    data2: usize,
}

impl Default for App {
    fn default() -> App {
        App {
            pending_command: false,
            write_len: 0,
            command: TextScreenCommand::Idle,
            data1: 1,
            data2: 0,
        }
    }
}

pub struct TextScreen<'a> {
    text_screen: &'a dyn hil::text_screen::TextScreen<'static>,
    apps: Grant<App, UpcallCount<1>, AllowRoCount<{ ro_allow::COUNT }>, AllowRwCount<0>>,
    current_app: OptionalCell<ProcessId>,
    buffer: TakeCell<'static, [u8]>,
}

impl<'a> TextScreen<'a> {
    pub fn new(
        text_screen: &'static dyn hil::text_screen::TextScreen,
        buffer: &'static mut [u8],
        grant: Grant<App, UpcallCount<1>, AllowRoCount<{ ro_allow::COUNT }>, AllowRwCount<0>>,
    ) -> TextScreen<'a> {
        TextScreen {
            text_screen: text_screen,
            apps: grant,
            current_app: OptionalCell::empty(),
            buffer: TakeCell::new(buffer),
        }
    }

    fn enqueue_command(
        &self,
        command: TextScreenCommand,
        data1: usize,
        data2: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        let res = self
            .apps
            .enter(processid, |app, _| {
                if self.current_app.is_none() {
                    self.current_app.set(processid);
                    app.data1 = data1;
                    app.data2 = data2;
                    app.command = command;
                    Ok(true)
                } else {
                    if app.pending_command {
                        Err(ErrorCode::BUSY)
                    } else {
                        app.pending_command = true;
                        app.command = command;
                        app.data1 = data1;
                        app.data2 = data2;
                        Ok(false)
                    }
                }
            })
            .map_err(ErrorCode::from);
        let res = match res {
            Ok(value) => value,
            Err(err) => Err(err),
        };
        match res {
            Ok(execute_now) => {
                if execute_now {
                    match self.do_command() {
                        Ok(()) => CommandReturn::success(),
                        Err(err) => {
                            self.current_app.clear();
                            CommandReturn::failure(err)
                        }
                    }
                } else {
                    CommandReturn::success()
                }
            }
            Err(err) => CommandReturn::failure(err),
        }
    }

    fn do_command(&self) -> Result<(), ErrorCode> {
        let mut run_next = false;
        let res = self.current_app.map_or(Err(ErrorCode::FAIL), |app| {
            self.apps
                .enter(app, |app, kernel_data| match app.command {
                    TextScreenCommand::GetResolution => {
                        let (x, y) = self.text_screen.get_size();
                        app.pending_command = false;
                        let _ = kernel_data
                            .schedule_upcall(0, (kernel::errorcode::into_statuscode(Ok(())), x, y));
                        run_next = true;
                        Ok(())
                    }
                    TextScreenCommand::Display => self.text_screen.display_on(),
                    TextScreenCommand::NoDisplay => self.text_screen.display_off(),
                    TextScreenCommand::Blink => self.text_screen.blink_cursor_on(),
                    TextScreenCommand::NoBlink => self.text_screen.blink_cursor_off(),
                    TextScreenCommand::SetCursor => {
                        self.text_screen.set_cursor(app.data1, app.data2)
                    }
                    TextScreenCommand::NoCursor => self.text_screen.hide_cursor(),
                    TextScreenCommand::Write => {
                        if app.data1 > 0 {
                            app.write_len = app.data1;
                            let res = kernel_data
                                .get_readonly_processbuffer(ro_allow::SHARED)
                                .and_then(|shared| {
                                    shared.enter(|to_write_buffer| {
                                        self.buffer.take().map_or(Err(ErrorCode::BUSY), |buffer| {
                                            let len = cmp::min(app.write_len, buffer.len());
                                            for n in 0..len {
                                                buffer[n] = to_write_buffer[n].get();
                                            }
                                            match self.text_screen.print(buffer, len) {
                                                Ok(()) => Ok(()),
                                                Err((ecode, buffer)) => {
                                                    self.buffer.replace(buffer);
                                                    Err(ecode)
                                                }
                                            }
                                        })
                                    })
                                });
                            match res {
                                Ok(Ok(())) => Ok(()),
                                Ok(Err(err)) => Err(err),
                                Err(err) => err.into(),
                            }
                        } else {
                            Err(ErrorCode::NOMEM)
                        }
                    }
                    TextScreenCommand::Clear => self.text_screen.clear(),
                    TextScreenCommand::Home => self.text_screen.clear(),
                    TextScreenCommand::ShowCursor => self.text_screen.show_cursor(),
                    _ => Err(ErrorCode::NOSUPPORT),
                })
                .map_err(ErrorCode::from)
        });
        if run_next {
            self.run_next_command();
        }
        match res {
            Ok(value) => value,
            Err(err) => Err(err),
        }
    }

    fn run_next_command(&self) {
        // Check for pending events.
        for app in self.apps.iter() {
            let processid = app.processid();
            let current_command = app.enter(|app, _| {
                if app.pending_command {
                    app.pending_command = false;
                    self.current_app.set(processid);
                    true
                } else {
                    false
                }
            });
            if current_command {
                if self.do_command() != Ok(()) {
                    self.current_app.clear();
                } else {
                    break;
                }
            }
        }
    }

    fn schedule_callback(&self, data1: usize, data2: usize, data3: usize) {
        self.current_app.take().map(|processid| {
            let _ = self.apps.enter(processid, |app, kernel_data| {
                app.pending_command = false;
                kernel_data.schedule_upcall(0, (data1, data2, data3)).ok();
            });
        });
    }
}

impl<'a> SyscallDriver for TextScreen<'a> {
    fn command(
        &self,
        command_num: usize,
        data1: usize,
        data2: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        match command_num {
            // This driver exists.
            0 => CommandReturn::success(),
            // Get Resolution
            1 => self.enqueue_command(TextScreenCommand::GetResolution, data1, data2, processid),
            // Display
            2 => self.enqueue_command(TextScreenCommand::Display, data1, data2, processid),
            // No Display
            3 => self.enqueue_command(TextScreenCommand::NoDisplay, data1, data2, processid),
            // Blink
            4 => self.enqueue_command(TextScreenCommand::Blink, data1, data2, processid),
            // No Blink
            5 => self.enqueue_command(TextScreenCommand::NoBlink, data1, data2, processid),
            // Show Cursor
            6 => self.enqueue_command(TextScreenCommand::ShowCursor, data1, data2, processid),
            // No Cursor
            7 => self.enqueue_command(TextScreenCommand::NoCursor, data1, data2, processid),
            // Write
            8 => self.enqueue_command(TextScreenCommand::Write, data1, data2, processid),
            // Clear
            9 => self.enqueue_command(TextScreenCommand::Clear, data1, data2, processid),
            // Home
            10 => self.enqueue_command(TextScreenCommand::Home, data1, data2, processid),
            //Set Curosr
            11 => self.enqueue_command(TextScreenCommand::SetCursor, data1, data2, processid),
            // NOSUPPORT
            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.apps.enter(processid, |_, _| {})
    }
}

impl<'a> hil::text_screen::TextScreenClient for TextScreen<'a> {
    fn command_complete(&self, r: Result<(), ErrorCode>) {
        self.schedule_callback(kernel::errorcode::into_statuscode(r), 0, 0);
        self.run_next_command();
    }

    fn write_complete(&self, buffer: &'static mut [u8], len: usize, r: Result<(), ErrorCode>) {
        self.buffer.replace(buffer);
        self.schedule_callback(kernel::errorcode::into_statuscode(r), len, 0);
        self.run_next_command();
    }
}
