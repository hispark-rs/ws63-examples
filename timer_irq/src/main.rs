//! WS63 timer-interrupt example — validates the ws63-qemu interrupt controller.
//!
//! TIMER_0 fires periodically -> IRQ 26 (a standard `mie` bit) -> the CPU takes
//! a machine interrupt. The interrupt *controller* (unmasking IRQ 26, the
//! priority defaults, the global enable) is driven through `hisi_riscv_hal::interrupt`
//! — the mie-bit tier of the WS63 model. The trap *vector* is still local to the
//! example: it installs its OWN `mtvec` in direct mode (overriding hisi-riscv-rt's
//! weak cross-crate trap hooks would trip rustc's no_mangle-collision check).
//! The handler clears the timer and bumps a counter; `main` prints it over UART0
//! so each interrupt is visible on the QEMU console.
//!
//! End-to-end proof: timer down-counter -> mip[26] -> the hart takes the
//! interrupt -> our handler runs -> UART shows the tick count climbing.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::interrupt::{self, Interrupt};
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

// TIMER_0 registers (base 0x4400_2000, TIMER0 block at +0x100).
const TIMER0_LOAD: *mut u32 = 0x4400_2100 as *mut u32;
const TIMER0_CONTROL: *mut u32 = 0x4400_2110 as *mut u32;
const TIMER0_EOI: *mut u32 = 0x4400_2114 as *mut u32;

static mut TICKS: u32 = 0;

// Local trap vector (direct mode): save caller-saved regs, dispatch in Rust,
// restore, mret. Unique symbol names — no collision with hisi-riscv-rt.
core::arch::global_asm!(
    ".section .text.tirq, \"ax\"",
    ".balign 4",
    ".global tirq_trap",
    "tirq_trap:",
    "    addi sp, sp, -64",
    "    sw ra,  0(sp)",
    "    sw t0,  4(sp)",
    "    sw t1,  8(sp)",
    "    sw t2, 12(sp)",
    "    sw t3, 16(sp)",
    "    sw t4, 20(sp)",
    "    sw t5, 24(sp)",
    "    sw t6, 28(sp)",
    "    sw a0, 32(sp)",
    "    sw a1, 36(sp)",
    "    sw a2, 40(sp)",
    "    sw a3, 44(sp)",
    "    sw a4, 48(sp)",
    "    sw a5, 52(sp)",
    "    sw a6, 56(sp)",
    "    sw a7, 60(sp)",
    "    call tirq_handle",
    "    lw ra,  0(sp)",
    "    lw t0,  4(sp)",
    "    lw t1,  8(sp)",
    "    lw t2, 12(sp)",
    "    lw t3, 16(sp)",
    "    lw t4, 20(sp)",
    "    lw t5, 24(sp)",
    "    lw t6, 28(sp)",
    "    lw a0, 32(sp)",
    "    lw a1, 36(sp)",
    "    lw a2, 40(sp)",
    "    lw a3, 44(sp)",
    "    lw a4, 48(sp)",
    "    lw a5, 52(sp)",
    "    lw a6, 56(sp)",
    "    lw a7, 60(sp)",
    "    addi sp, sp, 64",
    "    mret",
);

unsafe extern "C" {
    fn tirq_trap();
}

#[unsafe(no_mangle)]
extern "C" fn tirq_handle() {
    // Clear the TIMER_0 interrupt (de-asserts IRQ 26 in the intc) + count it.
    unsafe {
        core::ptr::write_volatile(TIMER0_EOI, 1);
        TICKS = TICKS.wrapping_add(1);
    }
}

fn put_u32(uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>, mut n: u32) {
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
    uart.write(s);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    uart.write(b"\r\nWS63 timer-IRQ test (TIMER_0 -> IRQ 26)\r\n");

    unsafe {
        // Install our own direct-mode mtvec (low 2 bits = 0 => Direct).
        core::arch::asm!("csrw mtvec, {0}", in(reg) tirq_trap as *const () as usize);

        // Periodic timer: reload ~2.4M counts (~0.1 s at the modeled 24 MHz);
        // enable (CONTROL bit0), interrupt unmasked (bit3 = 0).
        core::ptr::write_volatile(TIMER0_LOAD, 2_400_000);
        core::ptr::write_volatile(TIMER0_CONTROL, 0x1);

        // Drive the interrupt controller via the HAL: default local priorities,
        // unmask TIMER_0 (IRQ 26, an `mie` bit), then the global MIE.
        interrupt::init();
        interrupt::enable(Interrupt::TIMER_INT0);
        interrupt::enable_global();
    }

    let mut last = 0u32;
    loop {
        let t = unsafe { core::ptr::read_volatile(&raw const TICKS) };
        if t != last {
            last = t;
            uart.write(b"timer irq #");
            put_u32(&uart, t);
            uart.write(b"\r\n");
            if t == 5 {
                uart.write(b"OK: timer interrupts delivered\r\n");
            }
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
