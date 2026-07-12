//! WS63 UART hello-world — HAL UART driver.

#![no_std]
#![no_main]

use hisi_panic_handler as _;
use hisi_hal::{
    peripherals::Peripherals,
    uart::{Config, Uart, UartClock},
};
use hisi_riscv_rt::entry;

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

    uart.write(b"\r\nHello from WS63 (HAL UART driver)!\r\n");

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
