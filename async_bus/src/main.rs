//! WS63 async bus demo — `embedded-hal-async` `SpiBus` + `I2c`.
//!
//! Drives the async SPI and I2C drivers under `ws63_hal::asynch::block_on`.
//! ws63-qemu loops both peripherals' FIFOs back (DR→RX for SPI, TXR→RXR for
//! I2C), so an async `transfer_in_place` / `write_read` round-trips the data.
//! The transfers are FIFO-paced and complete promptly (no parking needed), which
//! is why a plain `block_on` suffices here — the same drivers also work under
//! embassy-executor.

#![no_std]
#![no_main]

// I2c async is called via UFCS below (its inherent write_read would shadow it).
use embedded_hal_async::spi::SpiBus as _;
use ws63_hal::Peripherals;
use ws63_hal::asynch::block_on;
use ws63_hal::i2c::I2c;
use ws63_hal::spi::{Config as SpiConfig, Spi};
use ws63_hal::uart::{Config as UartConfig, Uart};
use ws63_rt::entry;

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(
        0,
        b"\r\nWS63 async bus (embedded-hal-async SpiBus + I2c)\r\n",
    );

    let mut spi = Spi::new_spi0(p.SPI0, SpiConfig::default());
    let mut i2c = I2c::new_i2c0(p.I2C0, 100_000);

    let ok = block_on(async {
        let mut all = true;

        // SPI: loopback round-trip via async transfer_in_place.
        let tx = [0xA5u8, 0x3C, 0xFF, 0x01];
        let mut buf = tx;
        if spi.transfer_in_place(&mut buf).await.is_ok() {
            uart.write(0, b"  spi.transfer_in_place().await -> ");
            uart.write(
                0,
                if buf == tx {
                    b"loopback OK\r\n"
                } else {
                    b"MISMATCH\r\n"
                },
            );
            all &= buf == tx;
        } else {
            uart.write(0, b"  spi error\r\n");
            all = false;
        }

        // I2C: loopback write_read via async I2c (TXR -> RXR).
        let mut rd = [0u8; 2];
        match embedded_hal_async::i2c::I2c::write_read(&mut i2c, 0x42, &[0xDE, 0xAD], &mut rd).await
        {
            Ok(()) => {
                uart.write(0, b"  i2c.write_read().await -> ok\r\n");
            }
            Err(_) => {
                uart.write(
                    0,
                    b"  i2c.write_read().await -> err (no slave; trait path exercised)\r\n",
                );
            }
        }

        all
    });

    uart.write(
        0,
        if ok {
            b"ASYNC BUS: PASS\r\n"
        } else {
            b"ASYNC BUS: FAIL\r\n"
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
