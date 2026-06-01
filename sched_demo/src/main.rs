//! WS63 cooperative-scheduler demo (`ws63-rf-rs::sched`).
//!
//! Proves the real scheduler works on `ws63-qemu`, end to end:
//! - **Context switching**: two worker tasks each increment their own counter
//!   five times, yielding between increments — they only make progress if the
//!   scheduler actually switches stacks/registers between them and `main`.
//! - **Blocking semaphore**: a consumer task `down()`s an initially-empty
//!   semaphore three times (it must *block* each time), and a producer task
//!   `up()`s it three times — the consumer's count reaching 3 proves the
//!   block/wake path (a task is parked and later resumed).
//!
//! This is the runtime that backs the WS63 OSAL contract
//! (`osal_kthread_create` / `osal_wait_*` / `osal_msleep`) for the vendor Wi-Fi
//! blob. Reports `SCHED DEMO: PASS` over UART0.

#![no_std]
#![no_main]

use core::ffi::c_void;
use portable_atomic::{AtomicU32, Ordering};
use ws63_hal::Peripherals;
use ws63_hal::uart::{Config, Uart};
use ws63_rf_rs::sched::{self, Semaphore};
use ws63_rt::entry;

static COUNTERS: [AtomicU32; 2] = [AtomicU32::new(0), AtomicU32::new(0)];
static DONE: AtomicU32 = AtomicU32::new(0);
static GOT: AtomicU32 = AtomicU32::new(0);
/// Producer→consumer handoff semaphore (starts empty, so the consumer blocks).
static SEM: Semaphore = Semaphore::new(0);

const ROUNDS: u32 = 5;
const ITEMS: u32 = 3;

extern "C" fn worker(arg: *mut c_void) -> *mut c_void {
    let idx = arg as usize;
    for _ in 0..ROUNDS {
        COUNTERS[idx].fetch_add(1, Ordering::Relaxed);
        sched::yield_now();
    }
    DONE.fetch_add(1, Ordering::Relaxed);
    core::ptr::null_mut()
}

extern "C" fn producer(_arg: *mut c_void) -> *mut c_void {
    for _ in 0..ITEMS {
        SEM.up();
        sched::yield_now();
    }
    DONE.fetch_add(1, Ordering::Relaxed);
    core::ptr::null_mut()
}

extern "C" fn consumer(_arg: *mut c_void) -> *mut c_void {
    for _ in 0..ITEMS {
        SEM.down(); // blocks until the producer up()s
        GOT.fetch_add(1, Ordering::Relaxed);
    }
    DONE.fetch_add(1, Ordering::Relaxed);
    core::ptr::null_mut()
}

/// Format a u32 as decimal into `buf`, returning the used slice.
fn u32dec(mut v: u32, buf: &mut [u8; 10]) -> &[u8] {
    let mut i = buf.len();
    if v == 0 {
        i -= 1;
        buf[i] = b'0';
    }
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    &buf[i..]
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    uart.write(0, b"\r\nWS63 cooperative scheduler demo\r\n");

    sched::init();
    // Pass the worker index as the (tag-only) arg pointer.
    sched::spawn(worker, core::ptr::without_provenance_mut::<c_void>(0), 0);
    sched::spawn(worker, core::ptr::without_provenance_mut::<c_void>(1), 0);
    sched::spawn(producer, core::ptr::null_mut(), 0);
    sched::spawn(consumer, core::ptr::null_mut(), 0);

    // Run the scheduler cooperatively from the main task until the 4 spawned
    // tasks finish (bounded so a bug can't hang the demo).
    let mut guard: u32 = 0;
    while DONE.load(Ordering::Relaxed) < 4 && guard < 1_000_000 {
        sched::yield_now();
        guard += 1;
    }

    let c0 = COUNTERS[0].load(Ordering::Relaxed);
    let c1 = COUNTERS[1].load(Ordering::Relaxed);
    let got = GOT.load(Ordering::Relaxed);
    let done = DONE.load(Ordering::Relaxed);
    let mut b = [0u8; 10];
    uart.write(0, b"worker0 counter = ");
    uart.write(0, u32dec(c0, &mut b));
    uart.write(0, b"\r\n");
    uart.write(0, b"worker1 counter = ");
    uart.write(0, u32dec(c1, &mut b));
    uart.write(0, b"\r\n");
    uart.write(0, b"semaphore items = ");
    uart.write(0, u32dec(got, &mut b));
    uart.write(0, b"\r\n");
    uart.write(0, b"tasks finished  = ");
    uart.write(0, u32dec(done, &mut b));
    uart.write(0, b"\r\n");

    let ok = c0 == ROUNDS && c1 == ROUNDS && got == ITEMS && done == 4;
    uart.write(
        0,
        if ok {
            b"SCHED DEMO: PASS\r\n"
        } else {
            b"SCHED DEMO: FAIL\r\n"
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
