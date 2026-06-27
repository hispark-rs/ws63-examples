//! WS63 DMA example: peripheral-paced DMA + mem-to-mem on the primary M_DMA.
//!
//! Runs on the `ws63-qemu` machine model and validates the DMA controller end
//! to end without real hardware. Like `uart_hello`, it does NOT init the clocks
//! — it touches only UART0 (print), the M_DMA controller and SPI0.
//!
//! Two transfers, each verified and reported over UART0 (115200 8N1):
//!
//! 1. **Peripheral loopback (MDMA channel 0).** A `mem -> SPI0_DR` DMA
//!    (`MemToPeripheral` flow control, `Spi0Tx` handshaking, destination held
//!    fixed at the SPI data register) pushes a buffer into SPI0's loopback
//!    FIFO; a `SPI0_DR -> mem` DMA (`PeripheralToMem`, `Spi0Rx`, source fixed)
//!    reads it back. The round-trip exercises the flow-control type and
//!    handshaking-ID decode and the fixed-address peripheral routing.
//!
//! 2. **Memory-to-memory (M_DMA channel 1).** A `mem -> mem` transfer on the
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
use hisi_riscv_hal::dma::{Dma0, DmaChannelConfig, DmaDriver, DmaInstance, DmaPeripheral};
use hisi_riscv_hal::spi::{Config as SpiConfig, Spi};
use hisi_riscv_hal::uart::{Config as UartConfig, Uart};
use hisi_riscv_rt::entry;

/// SPI0 data register (PAC `spi0.spi_dr`, offset 0x60). The DMA peripheral
/// endpoint for the loopback: a write pushes the SPI loopback FIFO, a read pops.
const SPI0_DR: u32 = 0x4402_0060;

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

/// Poll a controller's raw transfer-done mask for `phys_bit` (the controller's
/// physical channel index) until set, bounded so a stuck transfer can't hang.
///
/// This is the real-hardware polling pattern. On `ws63-qemu` the transfer runs
/// synchronously inside the channel-enable MMIO write, so the done bit is
/// already set on the first read and this returns immediately.
fn wait_done<T: DmaInstance>(dma: &DmaDriver<T>, phys_bit: u8) -> bool {
    let mask = 1u8 << phys_bit;
    let mut n = 1_000_000u32;
    while n > 0 {
        if dma.raw_interrupt_status().0 & mask != 0 {
            return true;
        }
        n -= 1;
    }
    false
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(0, b"\r\nWS63 DMA loopback test\r\n");

    // Reserve SPI0; its data register is the DMA peripheral endpoint. The QEMU
    // model treats SPI as a loopback FIFO (write pushes, read pops); the
    // constructor only programs control/baud registers, never the FIFO.
    let _spi = Spi::new_spi0(p.SPI0, SpiConfig::default());

    let mut ok = true;

    // ── Part 1: mem -> SPI0_DR -> mem peripheral DMA (MDMA channel 0) ───────
    let src: [u32; N] = [
        0x1100_0011,
        0x2200_0022,
        0x3300_0033,
        0x4400_0044,
        0x5500_0055,
        0x6600_0066,
        0x7700_0077,
        0x8800_0088,
    ];
    let dst: [u32; N] = [0u32; N];

    let mut dma = DmaDriver::<Dma0>::new_dma(p.DMA);
    dma.enable_controller();

    // TX: memory -> SPI0 data register (destination held fixed, Spi0Tx handshake).
    let tx_cfg = DmaChannelConfig::default().mem_to_peripheral(DmaPeripheral::Spi0Tx);
    dma.configure_channel(0, src.as_ptr() as u32, SPI0_DR, N as u16, &tx_cfg);
    let tx_done = wait_done(&dma, 0);
    dma.clear_transfer_interrupt(0);

    // RX: SPI0 data register -> memory (source held fixed, Spi0Rx handshake).
    let rx_cfg = DmaChannelConfig::default().peripheral_to_mem(DmaPeripheral::Spi0Rx);
    dma.configure_channel(0, SPI0_DR, dst.as_ptr() as u32, N as u16, &rx_cfg);
    let rx_done = wait_done(&dma, 0);
    dma.clear_transfer_interrupt(0);

    uart.write(0, b"part1 mem->SPI0->mem (MDMA ch0): ");
    let mut part1 = tx_done && rx_done;
    for (i, &want) in src.iter().enumerate() {
        // Volatile: the DMA wrote `dst` behind the compiler's back.
        let got = unsafe { core::ptr::read_volatile(dst.as_ptr().add(i)) };
        if got != want {
            part1 = false;
            uart.write(0, b"\r\n  mismatch @");
            uart.write(0, &hex8(i as u32));
            uart.write(0, b" got=");
            uart.write(0, &hex8(got));
            uart.write(0, b" want=");
            uart.write(0, &hex8(want));
        }
    }
    uart.write(0, if part1 { b" OK\r\n" } else { b" FAIL\r\n" });
    ok &= part1;

    // ── Part 2: mem -> mem on the primary M_DMA (channel 1) ─────────────────
    let src2: [u32; N] = [
        0xaaaa_0001,
        0xaaaa_0002,
        0xaaaa_0003,
        0xaaaa_0004,
        0xaaaa_0005,
        0xaaaa_0006,
        0xaaaa_0007,
        0xaaaa_0008,
    ];
    let dst2: [u32; N] = [0u32; N];

    // The secure DMA (SDMA @0x520A_0000) is NEVER provisioned on WS63 silicon
    // (CONFIG_DMA_SUPPORT_SMDMA unset, g_sdma_base_addr unassigned): a transfer
    // there stalls an AXI beat forever and drops the debug link. Vendor mem->mem
    // always uses the primary M_DMA, so this runs on Dma0 channel 1 (channel 0
    // was used by part 1).
    dma.configure_channel(
        1,
        src2.as_ptr() as u32,
        dst2.as_ptr() as u32,
        N as u16,
        &DmaChannelConfig::default(),  // default flow control = mem -> mem
    );
    let s_done = wait_done(&dma, 1);
    dma.clear_transfer_interrupt(1);

    uart.write(0, b"part2 mem->mem (MDMA ch1): ");
    let mut part2 = s_done;
    for (i, &want) in src2.iter().enumerate() {
        let got = unsafe { core::ptr::read_volatile(dst2.as_ptr().add(i)) };
        if got != want {
            part2 = false;
        }
    }
    uart.write(0, if part2 { b" OK\r\n" } else { b" FAIL\r\n" });
    ok &= part2;

    if ok {
        uart.write(0, b"DMA LOOPBACK TEST: PASS\r\n");
    } else {
        uart.write(0, b"DMA LOOPBACK TEST: FAIL\r\n");
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
