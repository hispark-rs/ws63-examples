//! WS63 I2C bus-scan example.
//!
//! Probes 7-bit addresses 0x08..=0x77 on I2C0 (100 kHz) with a zero-length write
//! (START + address + STOP) and reports which ones ACK. On real hardware this
//! lists the connected devices; under ws63-qemu (no real slave) it exercises the
//! I2C driver + the bounded-timeout/NACK path end-to-end without hanging.

#![no_std]
#![no_main]

use ws63_hal::Peripherals;
use ws63_hal::i2c::I2c;
use ws63_hal::uart::{Config as UartConfig, Uart};
use ws63_rt::entry;

/// Write `byte` as two uppercase hex digits over UART0 (generic over the instance).
fn write_hex2<T>(uart: &Uart<'_, T>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let out = [HEX[(byte >> 4) as usize], HEX[(byte & 0xF) as usize]];
    uart.write(0, &out);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(0, b"\r\nWS63 I2C scan (I2C0, 100 kHz, addr 0x08..0x77)\r\n");

    let mut i2c = I2c::new_i2c0(p.I2C0, 100_000);

    let mut found = 0u32;
    for addr in 0x08u8..=0x77 {
        if i2c.write(addr, &[]).is_ok() {
            uart.write(0, b"  found device at 0x");
            write_hex2(&uart, addr);
            uart.write(0, b"\r\n");
            found += 1;
        }
    }

    uart.write(
        0,
        if found == 0 {
            b"  no devices acked\r\n"
        } else {
            b"  scan done\r\n"
        },
    );

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
