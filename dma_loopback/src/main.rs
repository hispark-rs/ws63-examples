//! WS63 DMA example: peripheral-paced DMA + mem-to-mem on the primary M_DMA.
//!
//! Runs on the `ws63-qemu` machine model and validates the DMA controller end
//! to end without real hardware. Like `uart_hello`, it does NOT init the clocks
//! — it touches only UART0 (print), the M_DMA controller and SPI0.
//!
//! Two transfers, each verified and reported over UART0 (115200 8N1):
//!
//! 1. **Peripheral loopback (MDMA channels 1+2).** `SpiDma::transfer_dma`
//!    drives MOSI from memory and captures MISO back to memory through SPI0's
//!    DMA handshake wiring. The round-trip exercises the public claimed-channel
//!    peripheral-DMA API.
//!
//! 2. **Memory-to-memory (M_DMA channel 0).** A `mem -> mem` transfer on the
//!    primary controller. (The secure DMA / SDMA @0x520A_0000 is never
//!    provisioned on WS63 silicon — a transfer there stalls AXI and hangs the
//!    bus — so vendor mem->mem always uses the primary M_DMA.)
//!
//! Completion is observed by polling the raw interrupt-status register
//! (`dmac_ori_int_st`), so no DMA interrupt wiring is required.
//!
//! Scope note: the `ws63-qemu` DMA model services every queued beat
//! immediately (MMIO-aware copy). It validates flow-control/handshaking-field
//! decode, transfer direction, addressing and transfer-complete signalling —
//! NOT cycle-accurate DMA-request pacing or FIFO back-pressure.

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::dma::{Dma0, DmaDriver};
use hisi_riscv_hal::spi::{Config as SpiConfig, Spi};
use hisi_riscv_hal::uart::{Config as UartConfig, Uart};
use hisi_riscv_rt::entry;

const N: usize = 8;

/// Render a u32 as 8 lowercase hex digits.
fn hex8(n: u32) -> [u8; 8] {
    let mut buf = [0u8; 8];
    let mut i = 0;
    while i < 8 {
        let nib = (n >> ((7 - i) * 4)) & 0xf;
        buf[i] = if nib < 10 {
            b'0' + nib as u8
        } else {
            b'a' + (nib - 10) as u8
        };
        i += 1;
    }
    buf
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(b"\r\nWS63 DMA loopback test\r\n");

    // Reserve SPI0; its data register is the DMA peripheral endpoint. The QEMU
    // model treats SPI as a loopback FIFO (write pushes, read pops); the
    // constructor only programs control/baud registers, never the FIFO.
    let spi = Spi::new_spi0(p.SPI0, SpiConfig::default());

    let mut ok = true;

    let dma = DmaDriver::<Dma0>::new_dma(p.DMA);
    let chs = dma.split_channels().expect("DMA channels already claimed");

    // ── Part 1: mem -> mem on the primary M_DMA (channel 0) ─────────────────
    #[repr(C, align(32))]
    struct Words([u32; N]);
    static SRC2: Words = Words([
        0xaaaa_0001,
        0xaaaa_0002,
        0xaaaa_0003,
        0xaaaa_0004,
        0xaaaa_0005,
        0xaaaa_0006,
        0xaaaa_0007,
        0xaaaa_0008,
    ]);
    static mut DST2: Words = Words([0u32; N]);
    // SAFETY: this example is single-threaded and owns the DMA destination buffer.
    let dst2: &'static mut [u32] = unsafe { &mut (*core::ptr::addr_of_mut!(DST2)).0 };

    // The secure DMA (SDMA @0x520A_0000) is NEVER provisioned on WS63 silicon
    // (CONFIG_DMA_SUPPORT_SMDMA unset, g_sdma_base_addr unassigned): a transfer
    // there stalls an AXI beat forever and drops the debug link. Vendor mem->mem
    // always uses the primary M_DMA, so this runs on Dma0 channel 0.
    let transfer = dma
        .start_mem_to_mem(chs.ch0, &SRC2.0[..], dst2)
        .expect("DMA mem-to-mem start failed");
    let (dma, _ch0, _src2, dst2) = transfer.wait().expect("DMA mem-to-mem wait failed");

    uart.write(b"part1 mem->mem (MDMA ch0): ");
    let mut part2 = true;
    for (i, &want) in SRC2.0.iter().enumerate() {
        let got = unsafe { core::ptr::read_volatile(dst2.as_ptr().add(i)) };
        if got != want {
            part2 = false;
        }
    }
    uart.write(if part2 { b" OK\r\n" } else { b" FAIL\r\n" });
    ok &= part2;

    // ── Part 2: SPI0 full-duplex peripheral DMA (MDMA channels 1+2) ──────────
    #[repr(C, align(32))]
    struct Bytes([u8; N]);
    static SRC: Bytes = Bytes([0x11, 0x22, 0x33, 0x44, 0x55, 0x66, 0x77, 0x88]);
    static mut DST: Bytes = Bytes([0u8; N]);
    // SAFETY: this example is single-threaded and owns the SPI DMA RX buffer.
    let dst: &'static mut [u8] = unsafe { &mut (*core::ptr::addr_of_mut!(DST)).0 };

    let mut spi_dma = spi.with_dma(dma);
    let (dst, _src) = spi_dma
        .transfer_dma(chs.ch1, chs.ch2, dst, &SRC.0[..])
        .expect("SPI DMA transfer failed");

    uart.write(b"part2 SPI0 DMA loopback (MDMA ch1+ch2): ");
    let mut part1 = true;
    for (i, &want) in SRC.0.iter().enumerate() {
        let got = unsafe { core::ptr::read_volatile(dst.as_ptr().add(i)) };
        if got != want {
            part1 = false;
            uart.write(b"\r\n  mismatch @");
            uart.write(&hex8(i as u32));
            uart.write(b" got=");
            uart.write(&hex8(got as u32));
            uart.write(b" want=");
            uart.write(&hex8(want as u32));
        }
    }
    uart.write(if part1 { b" OK\r\n" } else { b" FAIL\r\n" });
    ok &= part1;

    if ok {
        uart.write(b"DMA LOOPBACK TEST: PASS\r\n");
    } else {
        uart.write(b"DMA LOOPBACK TEST: FAIL\r\n");
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
