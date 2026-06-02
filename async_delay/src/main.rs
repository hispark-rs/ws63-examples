//! WS63 async example — `embedded-hal-async` `DelayNs` on a hardware TIMER.
//!
//! `ws63_hal::timer::AsyncDelay` arms TIMER0 one-shot and parks the task until
//! the timer's completion IRQ (26) fires; `ws63_hal::asynch::block_on` runs the
//! future, sleeping the core with `wfi` between polls. This exercises the full
//! async path on ws63-qemu: arm → `wfi` → TIMER IRQ → trap → `timer::on_interrupt`
//! → waker → re-poll → future completes.
//!
//! The HAL async drivers deliberately do NOT install a trap handler, so this
//! example owns its `mtvec` (direct mode) and routes TIMER_INT0 to
//! `timer::on_interrupt(0)` — exactly like the blocking `timer_irq` example.

#![no_std]
#![no_main]

use embedded_hal_async::delay::DelayNs;
use ws63_hal::Peripherals;
use ws63_hal::asynch::block_on;
use ws63_hal::interrupt;
use ws63_hal::timer::{self, AsyncDelay};
use ws63_hal::uart::{Config, Uart};
use ws63_rt::entry;

// Direct-mode trap: save caller-saved regs, dispatch in Rust, restore, mret.
core::arch::global_asm!(
    ".section .text.atrap, \"ax\"",
    ".balign 4",
    ".global atrap",
    "atrap:",
    "    addi sp, sp, -64",
    "    sw ra,0(sp)",
    "    sw t0,4(sp)",
    "    sw t1,8(sp)",
    "    sw t2,12(sp)",
    "    sw t3,16(sp)",
    "    sw t4,20(sp)",
    "    sw t5,24(sp)",
    "    sw t6,28(sp)",
    "    sw a0,32(sp)",
    "    sw a1,36(sp)",
    "    sw a2,40(sp)",
    "    sw a3,44(sp)",
    "    sw a4,48(sp)",
    "    sw a5,52(sp)",
    "    sw a6,56(sp)",
    "    sw a7,60(sp)",
    "    call atrap_handle",
    "    lw ra,0(sp)",
    "    lw t0,4(sp)",
    "    lw t1,8(sp)",
    "    lw t2,12(sp)",
    "    lw t3,16(sp)",
    "    lw t4,20(sp)",
    "    lw t5,24(sp)",
    "    lw t6,28(sp)",
    "    lw a0,32(sp)",
    "    lw a1,36(sp)",
    "    lw a2,40(sp)",
    "    lw a3,44(sp)",
    "    lw a4,48(sp)",
    "    lw a5,52(sp)",
    "    lw a6,56(sp)",
    "    lw a7,60(sp)",
    "    addi sp, sp, 64",
    "    mret",
);

unsafe extern "C" {
    fn atrap();
}

const TIMER_INT0: u32 = 26;

#[unsafe(no_mangle)]
extern "C" fn atrap_handle() {
    let mcause: u32;
    unsafe { core::arch::asm!("csrr {0}, mcause", out(reg) mcause) };
    if (mcause & 0x8000_0000) != 0 && (mcause & 0xFFF) == TIMER_INT0 {
        timer::on_interrupt(0); // EOI + stop one-shot + wake the AsyncDelay future
    }
}

fn put_u32(uart: &Uart<'_, ws63_hal::peripherals::Uart0<'_>>, mut n: u32) {
    let mut buf = [0u8; 10];
    let s: &[u8] = if n == 0 {
        buf[0] = b'0';
        &buf[..1]
    } else {
        let mut i = buf.len();
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        &buf[i..]
    };
    uart.write(0, s);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    uart.write(
        0,
        b"\r\nWS63 async delay (embedded-hal-async DelayNs on TIMER0)\r\n",
    );

    let mut delay = AsyncDelay::new(p.TIMER, 0);

    unsafe {
        core::arch::asm!("csrw mtvec, {0}", in(reg) atrap as *const () as usize); // direct mode
        interrupt::init();
        interrupt::enable_global();
    }

    block_on(async {
        for i in 0..5u32 {
            delay.delay_ms(20).await;
            uart.write(0, b"async tick #");
            put_u32(&uart, i + 1);
            uart.write(0, b"\r\n");
        }
        uart.write(0, b"ASYNC DELAY: PASS\r\n");
    });

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
