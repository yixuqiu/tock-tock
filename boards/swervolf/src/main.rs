// Licensed under the Apache License, Version 2.0 or the MIT License.
// SPDX-License-Identifier: Apache-2.0 OR MIT
// Copyright Tock Contributors 2022.

//! Board file for SweRVolf RISC-V development platform.
//!
//! - <https://github.com/chipsalliance/Cores-SweRVolf>
//!

#![no_std]
// Disable this attribute when documenting, as a workaround for
// https://github.com/rust-lang/rust/issues/62184.
#![cfg_attr(not(doc), no_main)]

use core::ptr::{addr_of, addr_of_mut};

use capsules_core::virtualizers::virtual_alarm::{MuxAlarm, VirtualMuxAlarm};
use kernel::capabilities;
use kernel::component::Component;
use kernel::hil;
use kernel::platform::{KernelResources, SyscallDriverLookup};
use kernel::scheduler::cooperative::CooperativeSched;
use kernel::utilities::registers::interfaces::ReadWriteable;
use kernel::{create_capability, debug, static_init};
use rv32i::csr;
use swervolf_eh1::chip::SweRVolfDefaultPeripherals;

pub mod io;

pub const NUM_PROCS: usize = 4;
//
// Actual memory for holding the active process structures. Need an empty list
// at least.
static mut PROCESSES: [Option<&'static dyn kernel::process::Process>; NUM_PROCS] =
    [None; NUM_PROCS];

// Reference to the chip for panic dumps.
static mut CHIP: Option<&'static swervolf_eh1::chip::SweRVolf<SweRVolfDefaultPeripherals>> = None;
// Static reference to process printer for panic dumps.
static mut PROCESS_PRINTER: Option<&'static capsules_system::process_printer::ProcessPrinterText> =
    None;

// How should the kernel respond when a process faults.
const FAULT_RESPONSE: capsules_system::process_policies::PanicFaultPolicy =
    capsules_system::process_policies::PanicFaultPolicy {};

/// Dummy buffer that causes the linker to reserve enough space for the stack.
#[no_mangle]
#[link_section = ".stack_buffer"]
pub static mut STACK_MEMORY: [u8; 0x900] = [0; 0x900];

/// A structure representing this platform that holds references to all
/// capsules for this platform. We've included an alarm and console.
struct SweRVolf {
    console: &'static capsules_core::console::Console<'static>,
    alarm: &'static capsules_core::alarm::AlarmDriver<
        'static,
        VirtualMuxAlarm<'static, swervolf_eh1::syscon::SysCon<'static>>,
    >,
    scheduler: &'static CooperativeSched<'static>,
    scheduler_timer: &'static swerv::eh1_timer::Timer<'static>,
}

/// Mapping of integer syscalls to objects that implement syscalls.
impl SyscallDriverLookup for SweRVolf {
    fn with_driver<F, R>(&self, driver_num: usize, f: F) -> R
    where
        F: FnOnce(Option<&dyn kernel::syscall::SyscallDriver>) -> R,
    {
        match driver_num {
            capsules_core::console::DRIVER_NUM => f(Some(self.console)),
            capsules_core::alarm::DRIVER_NUM => f(Some(self.alarm)),
            _ => f(None),
        }
    }
}

impl KernelResources<swervolf_eh1::chip::SweRVolf<'static, SweRVolfDefaultPeripherals<'static>>>
    for SweRVolf
{
    type SyscallDriverLookup = Self;
    type SyscallFilter = ();
    type ProcessFault = ();
    type Scheduler = CooperativeSched<'static>;
    type SchedulerTimer = swerv::eh1_timer::Timer<'static>;
    type WatchDog = ();
    type ContextSwitchCallback = ();

    fn syscall_driver_lookup(&self) -> &Self::SyscallDriverLookup {
        self
    }
    fn syscall_filter(&self) -> &Self::SyscallFilter {
        &()
    }
    fn process_fault(&self) -> &Self::ProcessFault {
        &()
    }
    fn scheduler(&self) -> &Self::Scheduler {
        self.scheduler
    }
    fn scheduler_timer(&self) -> &Self::SchedulerTimer {
        self.scheduler_timer
    }
    fn watchdog(&self) -> &Self::WatchDog {
        &()
    }
    fn context_switch_callback(&self) -> &Self::ContextSwitchCallback {
        &()
    }
}

/// Main function called after RAM initialized.
#[no_mangle]
pub unsafe fn main() {
    // only machine mode
    rv32i::configure_trap_handler();

    let peripherals = static_init!(
        SweRVolfDefaultPeripherals,
        SweRVolfDefaultPeripherals::new()
    );

    // initialize capabilities
    let process_mgmt_cap = create_capability!(capabilities::ProcessManagementCapability);
    let memory_allocation_cap = create_capability!(capabilities::MemoryAllocationCapability);

    let main_loop_cap = create_capability!(capabilities::MainLoopCapability);

    let board_kernel = static_init!(kernel::Kernel, kernel::Kernel::new(&*addr_of!(PROCESSES)));

    // Configure kernel debug gpios as early as possible
    kernel::debug::assign_gpios(None, None, None);

    // Create a shared UART channel for the console and for kernel debug.
    let uart_mux = components::console::UartMuxComponent::new(&peripherals.uart, 115200)
        .finalize(components::uart_mux_component_static!());

    let mtimer = static_init!(
        swervolf_eh1::syscon::SysCon,
        swervolf_eh1::syscon::SysCon::new()
    );

    // Create a shared virtualization mux layer on top of a single hardware
    // alarm.
    let mux_alarm = static_init!(
        MuxAlarm<'static, swervolf_eh1::syscon::SysCon>,
        MuxAlarm::new(mtimer)
    );
    hil::time::Alarm::set_alarm_client(mtimer, mux_alarm);

    // Alarm
    let virtual_alarm_user = static_init!(
        VirtualMuxAlarm<'static, swervolf_eh1::syscon::SysCon>,
        VirtualMuxAlarm::new(mux_alarm)
    );
    virtual_alarm_user.setup();

    let alarm = static_init!(
        capsules_core::alarm::AlarmDriver<
            'static,
            VirtualMuxAlarm<'static, swervolf_eh1::syscon::SysCon>,
        >,
        capsules_core::alarm::AlarmDriver::new(
            virtual_alarm_user,
            board_kernel.create_grant(capsules_core::alarm::DRIVER_NUM, &memory_allocation_cap)
        )
    );
    hil::time::Alarm::set_alarm_client(virtual_alarm_user, alarm);

    let chip = static_init!(
        swervolf_eh1::chip::SweRVolf<
            SweRVolfDefaultPeripherals,
        >,
        swervolf_eh1::chip::SweRVolf::new(peripherals, mtimer)
    );
    CHIP = Some(chip);

    // Create a process printer for panic.
    let process_printer = components::process_printer::ProcessPrinterTextComponent::new()
        .finalize(components::process_printer_text_component_static!());
    PROCESS_PRINTER = Some(process_printer);

    // Need to enable all interrupts for Tock Kernel
    chip.enable_pic_interrupts();

    // enable interrupts globally, including timer0 (bit 29) and timer1 (bit 28)
    csr::CSR.mie.modify(
        csr::mie::mie::mext::SET
            + csr::mie::mie::msoft::SET
            + csr::mie::mie::mtimer::SET
            + csr::mie::mie::BIT28::SET
            + csr::mie::mie::BIT29::SET,
    );
    csr::CSR.mstatus.modify(csr::mstatus::mstatus::mie::SET);

    // Setup the console.
    let console = components::console::ConsoleComponent::new(
        board_kernel,
        capsules_core::console::DRIVER_NUM,
        uart_mux,
    )
    .finalize(components::console_component_static!());
    // Create the debugger object that handles calls to `debug!()`.
    components::debug_writer::DebugWriterComponent::new(uart_mux)
        .finalize(components::debug_writer_component_static!());

    debug!("SweRVolf initialisation complete.");
    debug!("Entering main loop.");

    // These symbols are defined in the linker script.
    extern "C" {
        /// Beginning of the ROM region containing app images.
        static _sapps: u8;
        /// End of the ROM region containing app images.
        static _eapps: u8;
        /// Beginning of the RAM region for app memory.
        static mut _sappmem: u8;
        /// End of the RAM region for app memory.
        static _eappmem: u8;
    }

    let scheduler =
        components::sched::cooperative::CooperativeComponent::new(&*addr_of!(PROCESSES))
            .finalize(components::cooperative_component_static!(NUM_PROCS));

    let swervolf = SweRVolf {
        console,
        alarm,
        scheduler,
        scheduler_timer: chip.get_scheduler_timer(),
    };

    kernel::process::load_processes(
        board_kernel,
        chip,
        core::slice::from_raw_parts(
            core::ptr::addr_of!(_sapps),
            core::ptr::addr_of!(_eapps) as usize - core::ptr::addr_of!(_sapps) as usize,
        ),
        core::slice::from_raw_parts_mut(
            core::ptr::addr_of_mut!(_sappmem),
            core::ptr::addr_of!(_eappmem) as usize - core::ptr::addr_of!(_sappmem) as usize,
        ),
        &mut *addr_of_mut!(PROCESSES),
        &FAULT_RESPONSE,
        &process_mgmt_cap,
    )
    .unwrap_or_else(|err| {
        debug!("Error loading processes!");
        debug!("{:?}", err);
    });

    board_kernel.kernel_loop(
        &swervolf,
        chip,
        None::<&kernel::ipc::IPC<0>>,
        &main_loop_cap,
    );
}
