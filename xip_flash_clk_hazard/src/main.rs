//! WS63 hazard demo: re-switching the flash clock while executing XIP from flash.
//!
//! This example runs XIP directly from the flash window (like every WS63 app:
//! its code is linked at `0x0023_0300`). It then writes `CLDO_CRG_CLK_SEL` bit 18
//! to switch the **flash controller** clock to the PLL — exactly what
//! `clock_init::init_clocks()` does, and exactly what an XIP app must NOT do.
//!
//! On real silicon this invalidates the SFC XIP read timing the instant the flash
//! clock changes, so the **very next instruction fetch from flash crashes** and the
//! core + RISC-V Debug Module hang (only the hisiflash UART download recovers it).
//! flashboot already set up XIP, so this is the hazard from issue #4.
//!
//! On `ws63-qemu` the model reproduces it: the flash clock switch from XIP disables
//! the flash XIP window, so the fetch after the switch faults. Expected output:
//!
//! ```text
//! XIP-HAZARD: before flash-clock switch
//! ```
//!
//! and then nothing — the `after` line never prints (it would on a model that gives
//! the silicon a false green). The smoke test asserts exactly this.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

/// CLDO_CRG_CLK_SEL — bit 18 switches the flash/SFC controller clock to the PLL.
const CLDO_CRG_CLK_SEL: *mut u32 = 0x4400_1134 as *mut u32;
const FLASH_CLK_SEL_BIT: u32 = 1 << 18;

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());

    uart.write(b"\r\nXIP-HAZARD: before flash-clock switch\r\n");

    // Switch the flash clock to the PLL *while executing XIP from flash*. On
    // silicon (and now in ws63-qemu) the next fetch from flash crashes.
    unsafe {
        let cur = core::ptr::read_volatile(CLDO_CRG_CLK_SEL);
        core::ptr::write_volatile(CLDO_CRG_CLK_SEL, cur | FLASH_CLK_SEL_BIT);
    }

    // Unreachable on real silicon / ws63-qemu: the fetch of this code (in flash)
    // faults. A model that lets this print is giving a dangerous false green.
    uart.write(b"XIP-HAZARD: after switch (BUG: should not appear)\r\n");

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
