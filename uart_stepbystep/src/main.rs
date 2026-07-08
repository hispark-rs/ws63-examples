//! WS63 UART hello — step-by-step multi-peripheral style.
//!
//! Demonstrates the explicit step-by-step construction where each peripheral
//! and config is bound to a named variable. Best for applications with multiple
//! peripherals, or when you want to see the type annotations.

#![no_std]
#![no_main]

use hisi_riscv_hal::{
    peripherals::Peripherals,
    uart::{BaudRate, Config, Uart, UartClock},
};
use hisi_riscv_rt::entry;

#[entry]
fn main() -> ! {
    // 1. Take peripheral ownership
    let p = Peripherals::take().unwrap();

    // 2. Declare the baud rate (compile-time const — zero runtime cost)
    let baud = BaudRate::BAUD_115200;

    // 3. Choose UART baud-base clock
    let clock = UartClock::Boot;

    // 4. Assemble the frame configuration
    let cfg = Config {
        baudrate: baud,
        clock,
        ..Config::default()
    };

    // 5. Construct the UART driver — this writes the hardware registers
    let uart = Uart::new_uart0(p.UART0, cfg);

    uart.write(b"\r\n[stepbystep] Hello from WS63 UART0 (step-by-step style)!\r\n");

    let mut tick: u32 = 0;
    loop {
        let mut buf = [0u8; 10];
        let mut i = buf.len();
        let mut n = tick;
        if n == 0 {
            i -= 1;
            buf[i] = b'0';
        } else {
            while n > 0 {
                i -= 1;
                buf[i] = b'0' + (n % 10) as u8;
                n /= 10;
            }
        }
        uart.write(b"tick ");
        uart.write(&buf[i..]);
        uart.write(b"\r\n");
        tick = tick.wrapping_add(1);
        for _ in 0..5_000_000 {
            core::hint::spin_loop();
        }
    }
}

use hisi_panic_handler as _;
