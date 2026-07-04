//! WS63 example that supplies its OWN `memory.x`.
//!
//! `hisi-riscv-rt`'s bundled memory.x is disabled (`default-features = false` in
//! Cargo.toml); this crate's build.rs puts its own memory.x on the link search
//! path instead. To prove the per-example file is actually the one linked, the
//! example's memory.x defines `__custom_memory_marker = 0x00C0_FFEE`; we read
//! that symbol's address and print it. hisi-riscv-rt's memory.x does NOT define it, so
//! a value of 0x00C0_FFEE confirms the override worked (and the link only
//! succeeds at all because our memory.x supplied the MEMORY regions).

#![no_std]
#![no_main]

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;

// Defined only by THIS crate's memory.x (a linker symbol whose *address* is the
// marker value). Edition 2024 requires `unsafe extern`.
unsafe extern "C" {
    static __custom_memory_marker: u8;
}

/// Format `n` as 8 lowercase hex digits.
fn u32_hex(mut n: u32, buf: &mut [u8; 8]) -> &[u8] {
    for slot in buf.iter_mut().rev() {
        let d = (n & 0xf) as u8;
        *slot = if d < 10 { b'0' + d } else { b'a' + (d - 10) };
        n >>= 4;
    }
    &buf[..]
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());

    // The symbol's address IS the marker value (set via PROVIDE in memory.x).
    let marker = core::ptr::addr_of!(__custom_memory_marker) as usize as u32;

    uart.write(b"\r\ncustom_memory: booted from per-example memory.x\r\n");
    uart.write(b"custom_memory: marker=0x");
    let mut buf = [0u8; 8];
    uart.write(u32_hex(marker, &mut buf));
    uart.write(b"\r\n");
    if marker == 0x00C0_FFEE {
        uart.write(b"custom_memory: OK (per-example memory.x in effect)\r\n");
    } else {
        uart.write(b"custom_memory: FAIL (unexpected memory.x)\r\n");
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
