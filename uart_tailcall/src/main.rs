//! WS63 UART hello — tail-call declarative style.
//!
//! Demonstrates the compact one-shot construction where `Uart` is built inline
//! from `Peripherals::take()`, with all config expressed at the point of use.
//! Best for single-peripheral applications or quick prototyping.

#![no_std]
#![no_main]

use hisi_riscv_hal::{peripherals::Peripherals, uart::{BaudRate, Config, Uart, UartClock}};
use hisi_riscv_rt::entry;

#[entry]
fn main() -> ! {
    let uart = Uart::new_uart0(
        Peripherals::take().unwrap().UART0,
        Config {
            baudrate: BaudRate::BAUD_115200,
            clock: UartClock::Boot,
            ..Config::default()
        },
    );

    uart.write(b"\r\n[tailcall] Hello from WS63 UART0 (declarative style)!\r\n");

    let mut tick: u32 = 0;
    loop {
        let mut buf = [0u8; 10]; let mut i = buf.len(); let mut n = tick;
        if n == 0 { i -= 1; buf[i] = b'0'; }
        else { while n > 0 { i -= 1; buf[i] = b'0' + (n % 10) as u8; n /= 10; } }
        uart.write(b"tick "); uart.write(&buf[i..]); uart.write(b"\r\n");
        tick = tick.wrapping_add(1);
        for _ in 0..5_000_000 { core::hint::spin_loop(); }
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! { loop { core::hint::spin_loop(); } }
