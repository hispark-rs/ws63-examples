//! WS63 RF init smoke.
//!
//! This is the first real-silicon RF milestone binary. By default it is an RF1
//! image smoke: it proves the runtime image, `.wifi_pkt_ram`, panic path, and
//! RF porting crate fit together. With `--features full-init`, it pulls in the
//! vendor Wi-Fi init closure and calls `uapi_wifi_init`; that path currently
//! records the RF3 blocker, because stock `rust-lld` rejects HiSilicon
//! `R_RISCV_48_LLUI` relocation type 58 in the final executable link.

#![no_std]
#![no_main]

use hisi_panic_handler as _;
use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

type Errcode = u32;

#[cfg(feature = "full-init")]
unsafe extern "C" {
    fn uapi_wifi_init() -> Errcode;
}

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
    let uart = Uart::new_uart0(p.UART0, Config::default());

    uart.write(b"\r\nRF1_IMAGE_OK\r\n");
    uart.write(b"RF2_INIT_BEGIN\r\n");

    let ret = call_vendor_init(&uart);
    if ret == 0 {
        uart.write(b"RF2_INIT_OK\r\n");
    } else {
        uart.write(b"RF2_INIT_ERR:0x");
        uart.write(&hex8(ret));
        uart.write(b"\r\n");
    }

    loop {
        core::hint::spin_loop();
    }
}

#[cfg(feature = "full-init")]
fn call_vendor_init(_uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>) -> Errcode {
    // SAFETY: `uapi_wifi_init` is the vendor Wi-Fi init entry linked from the
    // WS63 RF blob delivery. The smoke binary has no Rust-side invariants beyond
    // providing the linker symbols, ROM table, and ws63-rf-rs porting layer.
    unsafe { uapi_wifi_init() }
}

#[cfg(not(feature = "full-init"))]
fn call_vendor_init(uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>) -> Errcode {
    uart.write(b"RF2_INIT_SKIPPED:full-init feature disabled\r\n");
    0xFFFF_FFFE
}
