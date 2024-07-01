// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! USB system call interface
//!
//! This capsule provides a system call interface to the USB controller.
//!
//! ## Instantiation
//!
//! The `UsbSyscallDriver` must be created by passing a reference to something
//! that implements `hil::usb::Client` (that is, something that is connected to
//! the USBC), as well as a `Grant` for managing application requests.  For
//! example:
//!
//! ```rust,ignore
//! # use kernel::static_init;
//!
//! // Configure the USB controller
//! let usb_client = static_init!(
//!     capsules::usb::usbc_client::Client<'static, sam4l::usbc::Usbc<'static>>,
//!     capsules::usb::usbc_client::Client::new(&sam4l::usbc::USBC));
//! sam4l::usbc::USBC.set_client(usb_client);
//!
//! // Configure the USB userspace driver
//! let usb_driver = static_init!(
//!     capsules::usb::usb_user::UsbSyscallDriver<'static,
//!         capsules::usb::usbc_client::Client<'static, sam4l::usbc::Usbc<'static>>>,
//!     capsules::usb::usb_user::UsbSyscallDriver::new(
//!         usb_client, board_kernel.create_grant(&grant_cap)));
//! ```

use kernel::grant::{AllowRoCount, AllowRwCount, Grant, UpcallCount};
use kernel::hil;
use kernel::syscall::{CommandReturn, SyscallDriver};
use kernel::utilities::cells::OptionalCell;
use kernel::{ErrorCode, ProcessId};

use capsules_core::driver;
pub const DRIVER_NUM: usize = driver::NUM::UsbUser as usize;

#[derive(Default)]
pub struct App {
    awaiting: Option<Request>,
}

pub struct UsbSyscallDriver<'a, C: hil::usb::Client<'a>> {
    usbc_client: &'a C,
    apps: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    serving_app: OptionalCell<ProcessId>,
}

impl<'a, C> UsbSyscallDriver<'a, C>
where
    C: hil::usb::Client<'a>,
{
    pub fn new(
        usbc_client: &'a C,
        apps: Grant<App, UpcallCount<1>, AllowRoCount<0>, AllowRwCount<0>>,
    ) -> Self {
        UsbSyscallDriver {
            usbc_client: usbc_client,
            apps: apps,
            serving_app: OptionalCell::empty(),
        }
    }

    fn serve_waiting_apps(&self) {
        if self.serving_app.is_some() {
            // An operation on the USBC client is in progress
            return;
        }

        // Find a waiting app and start its requested computation
        let mut found = false;
        for app in self.apps.iter() {
            app.enter(|app, upcalls| {
                if let Some(request) = app.awaiting {
                    found = true;
                    match request {
                        Request::EnableAndAttach => {
                            // Enable and attach (synchronously)
                            self.usbc_client.enable();
                            self.usbc_client.attach();

                            // Schedule a callback immediately
                            upcalls
                                .schedule_upcall(
                                    0,
                                    (kernel::errorcode::into_statuscode(Ok(())), 0, 0),
                                )
                                .ok();
                            app.awaiting = None;
                        }
                    }
                }
            });
            if found {
                break;
            }
        }

        if !found {
            // No userspace requests pending at this time
        }
    }
}

#[derive(Copy, Clone)]
enum Request {
    EnableAndAttach,
}

impl<'a, C> SyscallDriver for UsbSyscallDriver<'a, C>
where
    C: hil::usb::Client<'a>,
{
    fn command(
        &self,
        command_num: usize,
        _arg: usize,
        _: usize,
        processid: ProcessId,
    ) -> CommandReturn {
        match command_num {
            // This driver is present
            0 => CommandReturn::success(),

            // Enable USB controller, attach to bus, and service default control endpoint
            1 => {
                let result = self
                    .apps
                    .enter(processid, |app, _| {
                        if app.awaiting.is_some() {
                            // Each app may make only one request at a time
                            Err(ErrorCode::BUSY)
                        } else {
                            app.awaiting = Some(Request::EnableAndAttach);
                            Ok(())
                        }
                    })
                    .unwrap_or_else(|err| Err(err.into()));

                match result {
                    Ok(()) => {
                        self.serve_waiting_apps();
                        CommandReturn::success()
                    }
                    Err(e) => CommandReturn::failure(e),
                }
            }

            _ => CommandReturn::failure(ErrorCode::NOSUPPORT),
        }
    }

    fn allocate_grant(&self, processid: ProcessId) -> Result<(), kernel::process::Error> {
        self.apps.enter(processid, |_, _| {})
    }
}
