//! WS63 UART hello-world example.
//!
//! Prints a banner and a running tick counter over UART0 (115200 8N1).
//!
//! Designed to run on the `ws63-qemu` machine model: it deliberately does NOT
//! call `clock_init::init_clocks()`, so it touches only UART0 registers
//! (0x4401_0000) and needs no SYS_CTL0/CLDO_CRG modeling. On QEMU the output
//! appears on `-serial mon:stdio`; on real silicon you would init the clocks
//! first so the baud divisor matches the PLL.

#![no_std]
#![no_main]

use ws63_hal::Peripherals;
use ws63_hal::uart::{Config, Uart};
use ws63_rt::entry;

/// Format a u32 as decimal into `buf`, returning the used slice.
fn u32_to_dec(mut n: u32, buf: &mut [u8; 10]) -> &[u8] {
    if n == 0 {
        buf[0] = b'0';
        return &buf[..1];
    }
    let mut i = buf.len();
    while n > 0 {
        i -= 1;
        buf[i] = b'0' + (n % 10) as u8;
        n /= 10;
    }
    &buf[i..]
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());

    uart.write(0, b"\r\nHello from WS63 on QEMU!\r\n");
    uart.write(0, b"ws63-qemu: UART0 @ 0x44010000 is alive.\r\n");

    let mut tick: u32 = 0;
    loop {
        let mut buf = [0u8; 10];
        uart.write(0, b"tick ");
        uart.write(0, u32_to_dec(tick, &mut buf));
        uart.write(0, b"\r\n");
        tick = tick.wrapping_add(1);

        // Busy-wait between lines (~arbitrary at QEMU speed).
        for _ in 0..5_000_000 {
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
