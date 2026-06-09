//! WS63 phase-4 demo: the `ws63-rf-rs` porting layer in action.
//!
//! Builds on the phase-3 blob-link spike. It links the vendor Wi-Fi ROM-data
//! blob *through* `ws63-rf-rs` (the blob's external data symbols resolve to the
//! crate's `globals` module, the packet-RAM symbols to build.rs `--defsym`s),
//! and then exercises the **implemented** parts of the ws63-RF porting contract
//! and checks each works, reporting over UART0:
//!
//! 1. `osal_kmalloc` / `osal_kfree` — heap alloc is zero-initialised + R/W.
//! 2. `uapi_systick_get_ms` — monotonic (advances over a busy-wait).
//! 3. `memset_s` / `memcpy_s` — copy works, and an over-large copy is refused.
//! 4. `log_event_wifi_print2` — routes through the installed log sink.
//! 5. `g_buf_size` from the ROM-data blob == 40 — the blob linked via the crate.
//!
//! It does NOT run the Wi-Fi stack (that needs the vendor RF HAL + a scheduler;
//! see ws63-rf-rs docs / ROADMAP phase 4). This validates the porting layer is
//! real, and that a vendor blob links against it.

#![no_std]
#![no_main]

use core::cell::RefCell;
use core::ffi::c_void;
use critical_section::Mutex;
use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::uart::{Config, Uart};
use hisi_riscv_rt::entry;
use ws63_rf_rs::alloc::{osal_kfree, osal_kmalloc};
use ws63_rf_rs::log::{log_event_wifi_print2, memcpy_s, memset_s};
use ws63_rf_rs::uapi::uapi_systick_get_ms;

// A ROM-data global from libwifi_rom_data.a (blob init value 40), proving the
// blob is whole-archive linked through ws63-rf-rs.
unsafe extern "C" {
    static g_buf_size: u16;
}

// Log capture: ws63-rf-rs log sinks are bare `fn(&[u8])`, so route into a static
// buffer, then print it via the UART driver (avoids a captured UART handle).
static CAP: Mutex<RefCell<([u8; 128], usize)>> = Mutex::new(RefCell::new(([0u8; 128], 0)));

fn cap_sink(bytes: &[u8]) {
    critical_section::with(|cs| {
        let mut g = CAP.borrow_ref_mut(cs);
        let (buf, len) = &mut *g;
        for &b in bytes {
            if *len < buf.len() {
                buf[*len] = b;
                *len += 1;
            }
        }
    });
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
    uart.write(0, b"\r\nWS63 ws63-rf-rs porting-layer demo\r\n");

    ws63_rf_rs::set_log_sink(cap_sink);
    let mut ok = true;

    // 1. osal_kmalloc / osal_kfree: zero-initialised, read/write, freeable.
    let mem = osal_kmalloc(256);
    uart.write(0, b"osal_kmalloc(256)    = 0x");
    uart.write(0, &hex8(mem as u32));
    uart.write(0, b"\r\n");
    if mem.is_null() {
        ok = false;
    } else {
        let buf = mem as *mut u8;
        // SAFETY: 256 bytes owned by this allocation.
        unsafe {
            let zeroed = (0..256).all(|i| buf.add(i).read_volatile() == 0);
            for i in 0..256 {
                buf.add(i).write_volatile((i as u8) ^ 0x5a);
            }
            let rw = (0..256).all(|i| buf.add(i).read_volatile() == ((i as u8) ^ 0x5a));
            ok &= zeroed && rw;
        }
        osal_kfree(mem);
    }

    // 2. uapi_systick_get_ms monotonic across a busy-wait.
    let t1 = uapi_systick_get_ms();
    for _ in 0..300_000 {
        core::hint::spin_loop();
    }
    let t2 = uapi_systick_get_ms();
    ok &= t2 >= t1;
    uart.write(0, b"uapi_systick_get_ms  : t1=0x");
    uart.write(0, &hex8(t1 as u32));
    uart.write(0, b" t2=0x");
    uart.write(0, &hex8(t2 as u32));
    uart.write(0, b"\r\n");

    // 3. memset_s / memcpy_s: copy works; over-large copy is refused.
    let mut src = [0u8; 16];
    let mut dst = [0u8; 16];
    let m1 = memset_s(src.as_mut_ptr() as *mut c_void, 16, 0xAB, 16);
    let m2 = memcpy_s(
        dst.as_mut_ptr() as *mut c_void,
        16,
        src.as_ptr() as *const c_void,
        16,
    );
    let copied = dst.iter().all(|&b| b == 0xAB);
    // count (16) > dest_max (8) must be refused (non-zero).
    let m3 = memcpy_s(
        dst.as_mut_ptr() as *mut c_void,
        8,
        src.as_ptr() as *const c_void,
        16,
    );
    ok &= m1 == 0 && m2 == 0 && copied && m3 != 0;
    uart.write(
        0,
        if copied {
            b"memcpy_s/memset_s    : OK\r\n"
        } else {
            b"memcpy_s/memset_s    : FAIL\r\n"
        },
    );

    // 4. log_event_wifi_print2 routes through the sink.
    log_event_wifi_print2(c"blob log via ws63-rf-rs sink".as_ptr());
    let cap_len = critical_section::with(|cs| CAP.borrow_ref(cs).1);
    ok &= cap_len > 0;
    uart.write(0, b"log sink captured    : ");
    critical_section::with(|cs| {
        let g = CAP.borrow_ref(cs);
        let (buf, len) = &*g;
        uart.write(0, &buf[..*len]);
    });
    uart.write(0, b"\r\n");

    // 5. ROM-data blob linked through ws63-rf-rs (g_buf_size init = 40).
    let buf_size = unsafe { core::ptr::read_volatile(&raw const g_buf_size) };
    uart.write(0, b"g_buf_size (rom_data)= 0x");
    uart.write(0, &hex8(buf_size as u32));
    uart.write(0, b"\r\n");
    ok &= buf_size == 40;

    uart.write(
        0,
        if ok {
            b"RF PORT DEMO: PASS\r\n"
        } else {
            b"RF PORT DEMO: FAIL\r\n"
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
