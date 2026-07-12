//! WS63 system-reset example — validates `hisi_hal::System::software_reset` and
//! `reset_reason` end-to-end on ws63-qemu.
//!
//! On the first (cold) boot the reset-reason record is empty, so `reset_reason()`
//! returns `Unknown` and the demo calls `software_reset()`. ws63-qemu models the
//! GLB_CTL_M chip-reset trigger (0x40002110 bit 2) and records the soft-reset
//! cause across the reboot, so on the second boot `reset_reason()` returns
//! `Software` and the demo prints the success marker and stops — proving both the
//! reset trigger and the reset-reason decode work.
//!
//! End-to-end proof: banner prints -> "triggering software_reset()" -> the machine
//! reboots -> banner prints again -> "OK: software reset observed".

#![no_std]
#![no_main]

use hisi_hal::Peripherals;
use hisi_hal::system::{ResetReason, System};
use hisi_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

fn reason_str(r: ResetReason) -> &'static [u8] {
    match r {
        ResetReason::PowerOn => b"power-on",
        ResetReason::ExternalPin => b"external-pin",
        ResetReason::Watchdog => b"watchdog",
        ResetReason::Software => b"software",
        ResetReason::BrownOut => b"brown-out",
        ResetReason::Unknown => b"unknown (cold boot)",
    }
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    let system = System::new(p.SYS_CTL0, p.GLB_CTL_M, p.CLDO_CRG);

    uart.write(b"\r\nWS63 system-reset test\r\n");

    let reason = system.reset_reason();
    uart.write(b"reset reason: ");
    uart.write(reason_str(reason));
    uart.write(b"\r\n");

    if reason == ResetReason::Software {
        // Second boot: the soft reset we triggered was recorded and decoded.
        uart.write(b"OK: software reset observed\r\n");
        loop {
            core::hint::spin_loop();
        }
    }

    // First (cold) boot: trigger a software reset and never return.
    uart.write(b"cold boot -> triggering software_reset()\r\n");
    system.software_reset();
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
