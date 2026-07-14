//! WS63 software-interrupt routing diagnostic.
//!
//! This proves the `SYS_CTL1.SOFT_INT0` routing and records its actual `mcause`.
//! The named `SOFT_INT0` handler also verifies the runtime's direct-mode
//! interrupt table and PAC `device.x` symbol.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU32, Ordering};
use hisi_hal::Peripherals;
use hisi_hal::software_interrupt::SoftwareInterrupt0;
use hisi_hal::interrupt;
use hisi_hal::uart::{Config, Uart, UartClock};
use hisi_riscv_rt::entry;

static TRAP_COUNT: AtomicU32 = AtomicU32::new(0);
static LAST_MCAUSE: AtomicU32 = AtomicU32::new(0);

#[unsafe(no_mangle)]
extern "C" fn SOFT_INT0() {
    let mcause: u32;
    unsafe {
        core::arch::asm!("csrr {mcause}, mcause", mcause = out(reg) mcause, options(nomem, nostack));
    }
    LAST_MCAUSE.store(mcause, Ordering::Relaxed);
    TRAP_COUNT.store(
        TRAP_COUNT.load(Ordering::Relaxed).wrapping_add(1),
        Ordering::Relaxed,
    );

    SoftwareInterrupt0::clear_interrupt();
}

fn write_hex(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>, value: u32) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = *b"0x00000000";
    for i in 0..8 {
        output[9 - i] = HEX[((value >> (i * 4)) & 0xf) as usize];
    }
    uart.write(&output);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(
        p.UART0,
        Config {
            clock: UartClock::Boot,
            ..Config::default()
        },
    );
    let software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);

    uart.write(b"\r\nWS63 software IRQ diagnostic\r\n");

    unsafe {
        interrupt::enable_global();
    }

    uart.write(b"SOFT_INT_STS before set: ");
    write_hex(&uart, software_interrupt.is_pending() as u32);
    uart.write(b"\r\n");

    software_interrupt.pend();

    for _ in 0..5_000_000 {
        if TRAP_COUNT.load(Ordering::Relaxed) != 0 {
            break;
        }
        core::hint::spin_loop();
    }

    let count = TRAP_COUNT.load(Ordering::Relaxed);
    let mcause = LAST_MCAUSE.load(Ordering::Relaxed);
    uart.write(b"trap count: ");
    write_hex(&uart, count);
    uart.write(b" mcause: ");
    write_hex(&uart, mcause);
    uart.write(b" status after handler: ");
    write_hex(&uart, software_interrupt.is_pending() as u32);
    uart.write(b"\r\n");

    if count == 1 && mcause == 0x8000_0024 {
        uart.write(b"OK: SOFT_INT0 -> local IRQ 36\r\n");
    } else {
        uart.write(b"FAIL: unexpected software IRQ routing\r\n");
    }

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
