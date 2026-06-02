//! WS63 self-test that reports its result via RISC-V semihosting.
//!
//! Runs a few CPU-only invariants (M-extension multiply, F-extension hard-float
//! arithmetic over the ilp32f ABI, and the `mcycle` CSR advancing) and then
//! calls the semihosting `SYS_EXIT_EXTENDED` operation with exit code 0 on PASS
//! or 1 on FAIL. A panic exits with code 2.
//!
//! The point is CI ergonomics: run it under `qemu-system-riscv32 -M ws63
//! -semihosting -kernel semihost_selftest` and the QEMU *process exit code* is
//! the test result — no UART scraping required (see ws63-qemu scripts/run.sh
//! SEMIHOST=1 and scripts/smoke-test.sh). The `semihosting` module below is the
//! minimal, copyable helper; on real silicon (no semihosting host) the trap is
//! a no-op and `exit` just spins.

#![no_std]
#![no_main]

use ws63_rt::entry;

/// Minimal RISC-V semihosting helper (exit + console write).
mod semihosting {
    use core::arch::asm;

    const SYS_WRITE0: usize = 0x04;
    const SYS_EXIT_EXTENDED: usize = 0x20;
    const ADP_STOPPED_APPLICATION_EXIT: usize = 0x2_0026;

    /// Issue a semihosting call: a0 = operation, a1 = argument, returns a0.
    /// The magic sequence must be the exact three 32-bit (norvc) instructions,
    /// contiguous around `ebreak`, for QEMU to recognise it.
    #[inline(never)]
    unsafe fn call(op: usize, arg: usize) -> usize {
        let ret;
        unsafe {
            asm!(
                ".option push",
                ".option norvc",
                "slli x0, x0, 0x1f",
                "ebreak",
                "srai x0, x0, 0x7",
                ".option pop",
                inout("a0") op => ret,
                in("a1") arg,
                options(nostack, preserves_flags),
            );
        }
        ret
    }

    /// Write a NUL-terminated byte string to the host semihosting console.
    pub fn write0(s: &[u8]) {
        unsafe {
            call(SYS_WRITE0, s.as_ptr() as usize);
        }
    }

    /// Exit the emulator with `code` (0 = success). No-op trap on real hardware.
    pub fn exit(code: i32) -> ! {
        let block = [ADP_STOPPED_APPLICATION_EXIT, code as usize];
        unsafe {
            call(SYS_EXIT_EXTENDED, block.as_ptr() as usize);
        }
        loop {
            unsafe { asm!("wfi") };
        }
    }
}

/// Read the low 32 bits of the `mcycle` CSR.
fn rdcycle() -> u32 {
    let c: u32;
    unsafe {
        core::arch::asm!("csrr {0}, mcycle", out(reg) c, options(nomem, nostack));
    }
    c
}

/// CPU-only invariants exercising M / F / Zicsr+Zicntr. `black_box` stops the
/// optimiser folding these away so the actual instructions execute.
fn run_checks() -> bool {
    use core::hint::black_box;
    let mut ok = true;

    // M extension: integer multiply.
    ok &= black_box(123u32) * black_box(456u32) == 56_088;

    // F extension (hard-float, ilp32f): single-precision arithmetic.
    let x = black_box(2.0f32);
    ok &= x * x + 1.0 == 5.0;

    // Zicsr / Zicntr: mcycle advances across a busy loop.
    let c0 = rdcycle();
    let mut acc = 0u32;
    for i in 0..1000u32 {
        acc = acc.wrapping_add(black_box(i));
    }
    black_box(acc);
    ok &= rdcycle() != c0;

    ok
}

#[entry]
fn main() -> ! {
    if run_checks() {
        semihosting::write0(b"semihost_selftest: PASS\n\0");
        semihosting::exit(0);
    } else {
        semihosting::write0(b"semihost_selftest: FAIL\n\0");
        semihosting::exit(1);
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    semihosting::write0(b"semihost_selftest: PANIC\n\0");
    semihosting::exit(2);
}
