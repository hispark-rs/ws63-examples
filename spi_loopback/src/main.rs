//! WS63 SPI loopback example (blocking).
//!
//! Configures SPI0 (full-duplex, Mode0, 1 MHz) and round-trips a few bytes with
//! a blocking [`Spi::transfer`]. ws63-qemu loops SPI0's TX FIFO back to RX, so
//! the read buffer equals what was written; on real silicon, short MOSI↔MISO.
//!
//! `Spi::new_spi0` programs the **two-stage clock** (a CLDO_CRG divider sets the
//! 160 MHz SSI_CLK off the 480 MHz PLL, then SCKDV divides to SCK), so the
//! configured 1 MHz is honoured on hardware — see hisi-riscv-hal `spi.rs`.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::spi::{Config as SpiConfig, DataBits, Spi, SpiHz, SpiMode};
use hisi_riscv_hal::uart::{Config as UartConfig, Uart};
use hisi_riscv_rt::entry;

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(0, b"\r\nWS63 SPI loopback (SPI0, Mode0, 1 MHz)\r\n");

    let mut spi = Spi::new_spi0(
        p.SPI0,
        SpiConfig {
            frequency: SpiHz::ONE_MHZ,
            mode: SpiMode::Mode0,
            data_bits: DataBits::EIGHT,
        },
    );

    let tx = [0xA5u8, 0x3C, 0xFF, 0x01];
    let mut rx = [0u8; 4];
    match spi.transfer(&tx, &mut rx) {
        Ok(()) if rx == tx => uart.write(0, b"  SPI loopback OK\r\n"),
        Ok(()) => uart.write(0, b"  SPI loopback MISMATCH\r\n"),
        Err(_) => uart.write(0, b"  SPI error (timeout)\r\n"),
    }

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
