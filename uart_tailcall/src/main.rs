//! WS63 UART — Pll panic message test

#![no_std]
#![no_main]

use core::panic::PanicInfo;
use hisi_riscv_hal::{
    peripherals::Peripherals,
    uart::{BaudRate, Config, Uart, UartClock},
};
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
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    unsafe {
        let cken1 = 0x4400_1104 as *mut u32;
        core::ptr::write_volatile(cken1, core::ptr::read_volatile(cken1) | (1 << 18));
        let data = 0x4401_0000 as *mut u16;
        let st = 0x4401_0044 as *const u16;
        let msg = b"\r\n[PANIC] UartClock::Boot without clock_init\r\n";
        for &b in msg {
            while core::ptr::read_volatile(st) & 0x01 != 0 {
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(data, b as u16);
        }
    }
    for _ in 0..10_000_000 {
        core::hint::spin_loop();
    }
    loop {
        core::hint::spin_loop();
    }
}
