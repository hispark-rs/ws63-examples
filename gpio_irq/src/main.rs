//! WS63 GPIO-interrupt example — validates ws63-qemu *custom local interrupt*
//! (IRQ >= 32) delivery, the part that needs the target/riscv patch.
//!
//! GPIO0 pin0 is configured as an output with a rising-edge interrupt. ws63-qemu
//! models output->input loopback, so toggling the pin generates the edge that
//! raises GPIO_0 = IRQ 33. The riscv31 core takes it with mcause=33 and (here,
//! direct-mode mtvec) jumps to our trap handler, which reads mcause, clears the
//! GPIO edge latch + the pending bit, and counts. `main` prints the count.
//!
//! The interrupt *controller* — the local priority defaults, unmasking IRQ 33
//! via `LOCIEN`, the global enable, and the `LOCIPCLR` pending-clear — is driven
//! through `hisi_riscv_hal::interrupt`, exercising the custom-CSR (IRQ >= 32) tier of
//! the WS63 model. The trap *vector* stays local to the example (its own mtvec)
//! to avoid overriding hisi-riscv-rt's weak cross-crate trap hooks (rustc no_mangle
//! collision). Proves the LOCIEN-gated, mcause-32-72 delivery path end-to-end.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::interrupt::{self, Interrupt};
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

const GPIO0: usize = 0x4402_8000;
const GPIO_OEN: usize = GPIO0 + 0x04;
const GPIO_INT_EN: usize = GPIO0 + 0x0C;
const GPIO_INT_TYPE: usize = GPIO0 + 0x14;
const GPIO_INT_POL: usize = GPIO0 + 0x18;
const GPIO_INT_EOI: usize = GPIO0 + 0x2C;
const GPIO_DATA_SET: usize = GPIO0 + 0x30;
const GPIO_DATA_CLR: usize = GPIO0 + 0x34;

const GPIO0_IRQ: u32 = Interrupt::GPIO_INT0 as u32;

static mut COUNT: u32 = 0;

// Direct-mode trap vector: save caller-saved, dispatch in Rust, restore, mret.
core::arch::global_asm!(
    ".section .text.girq, \"ax\"",
    ".balign 4",
    ".global girq_trap",
    "girq_trap:",
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
    "    call girq_handle",
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
    fn girq_trap();
}

#[unsafe(no_mangle)]
extern "C" fn girq_handle() {
    let mcause: u32;
    unsafe { core::arch::asm!("csrr {0}, mcause", out(reg) mcause) };
    // Interrupt (MSB set) with cause = GPIO0 IRQ?
    if (mcause & 0x8000_0000) != 0 && (mcause & 0xFFF) == GPIO0_IRQ {
        unsafe {
            core::ptr::write_volatile(GPIO_INT_EOI as *mut u32, 1); // clear GPIO edge latch
            COUNT = COUNT.wrapping_add(1);
        }
        interrupt::clear_pending(Interrupt::GPIO_INT0); // LOCIPCLR via the HAL
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
    uart.write(0, s);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    uart.write(
        0,
        b"\r\nWS63 GPIO-IRQ test (GPIO0 pin0 -> IRQ 33, custom local)\r\n",
    );

    unsafe {
        core::arch::asm!("csrw mtvec, {0}", in(reg) girq_trap as *const () as usize); // direct mode
        core::ptr::write_volatile(GPIO_OEN as *mut u32, 0); // pin0 output
        core::ptr::write_volatile(GPIO_INT_TYPE as *mut u32, 1); // pin0 edge-triggered
        core::ptr::write_volatile(GPIO_INT_POL as *mut u32, 1); // pin0 rising edge
        core::ptr::write_volatile(GPIO_INT_EN as *mut u32, 1); // pin0 interrupt enabled
        // Drive the controller via the HAL: default local priorities, unmask
        // GPIO_INT0 (IRQ 33, a custom local interrupt via LOCIEN0 bit1), global MIE.
        interrupt::init();
        interrupt::enable(Interrupt::GPIO_INT0);
        interrupt::enable_global();
    }

    let mut last = 0u32;
    loop {
        unsafe {
            core::ptr::write_volatile(GPIO_DATA_CLR as *mut u32, 1); // pin0 low
            core::ptr::write_volatile(GPIO_DATA_SET as *mut u32, 1); // pin0 high -> rising edge -> IRQ 33
        }
        let c = unsafe { core::ptr::read_volatile(&raw const COUNT) };
        if c != last {
            last = c;
            uart.write(0, b"gpio irq #");
            put_u32(&uart, c);
            uart.write(0, b"\r\n");
            if c == 5 {
                uart.write(0, b"OK: custom local IRQ (>=32) delivered\r\n");
            }
        }
        for _ in 0..200_000 {
            core::hint::spin_loop();
        }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
