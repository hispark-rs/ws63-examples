//! WS63 I2C bus-scan example.
//!
//! Probes 7-bit addresses 0x08..=0x77 on I2C0 (100 kHz) with a zero-length write
//! (START + address + STOP) and reports which ones ACK. On real hardware this
//! lists the connected devices; under ws63-qemu (no real slave) it exercises the
//! I2C driver + the bounded-timeout/NACK path end-to-end without hanging.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::i2c::{I2c, Speed};
use hisi_riscv_hal::io_config::{IoConfigDriver, MuxFunction, UartPad};
use hisi_riscv_hal::uart::{Config as UartConfig, Uart, UartInstance};
use hisi_riscv_rt::entry;

/// Write `byte` as two uppercase hex digits over UART0 (generic over the instance).
fn write_hex2<T: UartInstance>(uart: &Uart<'_, T>, byte: u8) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let out = [HEX[(byte >> 4) as usize], HEX[(byte & 0xF) as usize]];
    uart.write(&out);
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(b"\r\nWS63 I2C scan (I2C0, 100 kHz, addr 0x08..0x77)\r\n");

    // The WS63 EVB routes I2C0 SCL/SDA through pads 15/16, function 2.
    let mut io = IoConfigDriver::new(p.IO_CONFIG);
    io.set_uart_mux(UartPad::Uart1Txd, MuxFunction::F2);
    io.set_uart_mux(UartPad::Uart1Rxd, MuxFunction::F2);

    let mut i2c = I2c::new_i2c0(p.I2C0, Speed::Standard);

    let mut found = 0u32;
    for addr in 0x08u8..=0x77 {
        if i2c.write(addr, &[]).is_ok() {
            uart.write(b"  found device at 0x");
            write_hex2(&uart, addr);
            uart.write(b"\r\n");
            found += 1;
        }
    }

    uart.write(if found == 0 {
        b"  no devices acked\r\n"
    } else {
        b"  scan done\r\n"
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
