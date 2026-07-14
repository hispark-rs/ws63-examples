//! WS63 RTOS preemption and floating-point context HIL.
//!
//! Two equal-priority tasks retain distinct values in caller-saved `ft0` while
//! TIMER_INT0 forces round-robin preemption. The timer handler deliberately
//! overwrites `ft0`, so this test only passes when the runtime trap frame saves
//! and restores all floating-point registers.

#![no_std]
#![no_main]

use core::cell::{Cell, UnsafeCell};
use core::ffi::c_void;
use core::num::{NonZeroU32, NonZeroUsize};

use critical_section::Mutex;
use hisi_alloc::CHeap;
use hisi_hal::Peripherals;
use hisi_hal::interrupt;
use hisi_hal::software_interrupt::SoftwareInterrupt0;
use hisi_hal::time::Instant;
use hisi_hal::timer::TimerAlarm0;
use hisi_hal::uart::{Config as UartConfig, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
use hisi_rf_rtos_driver::TaskConfig;
use hisi_riscv_rt::entry;

const HEAP_SIZE: usize = 32 * 1024;
const TASK_STACK_SIZE: usize = 4 * 1024;

#[repr(C, align(16))]
struct Arena(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: the arena is exclusively registered with HEAP before interrupts are
// enabled and is never accessed directly afterwards.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; HEAP_SIZE]));
static HEAP: CHeap = CHeap::empty();
static LOOPS: Mutex<Cell<[u32; 2]>> = Mutex::new(Cell::new([0; 2]));
static FAILURES: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

unsafe fn allocate(size: usize) -> *mut u8 {
    HEAP.allocate_zeroed(size, 16)
}

unsafe fn deallocate(pointer: *mut u8) {
    // SAFETY: hisi-rtos returns only live pointers allocated by HEAP.
    let _ = unsafe { HEAP.deallocate(pointer) };
}

fn monotonic_ms() -> u64 {
    // Instant::raw() is the 24 MHz TCXO counter, not microseconds.
    Instant::now().raw() / 24_000
}

#[cfg(target_arch = "riscv32")]
fn retain_ft0(expected: u32) -> u32 {
    let observed: u32;
    // SAFETY: the assembly touches only caller-saved t0/ft0 and does not access
    // memory. TIMER_INT0 must be able to interrupt this loop.
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +f",
            "fmv.w.x ft0, {expected}",
            "li t0, 2000000",
            "1:",
            "addi t0, t0, -1",
            "bnez t0, 1b",
            "fmv.x.w {observed}, ft0",
            ".option pop",
            expected = in(reg) expected,
            observed = lateout(reg) observed,
            out("t0") _,
            out("ft0") _,
            options(nomem, nostack),
        );
    }
    observed
}

fn worker(index: usize, expected: u32) -> ! {
    loop {
        if retain_ft0(expected) != expected {
            critical_section::with(|cs| {
                let failures = FAILURES.borrow(cs);
                failures.set(failures.get().saturating_add(1));
            });
        }
        critical_section::with(|cs| {
            let loops = LOOPS.borrow(cs);
            let mut values = loops.get();
            values[index] = values[index].saturating_add(1);
            loops.set(values);
        });
    }
}

extern "C" fn worker0(_: *mut c_void) -> *mut c_void {
    worker(0, 1.0_f32.to_bits())
}

extern "C" fn worker1(_: *mut c_void) -> *mut c_void {
    worker(1, 2.0_f32.to_bits())
}

#[unsafe(no_mangle)]
extern "C" fn TIMER_INT0() {
    TimerAlarm0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    // Deliberately clobber a caller-saved FPR after the runtime trap frame has
    // captured it. restore_fpu must undo this before the interrupted task runs.
    unsafe {
        core::arch::asm!(
            ".option push",
            ".option arch, +f",
            "fmv.w.x ft0, zero",
            ".option pop",
            out("ft0") _,
            options(nomem, nostack),
        );
    }
    hisi_rtos::on_timer_interrupt();
    hisi_rtos::interrupt_exit();
}

#[unsafe(no_mangle)]
extern "C" fn SOFT_INT0() {
    SoftwareInterrupt0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    hisi_rtos::on_software_interrupt();
    hisi_rtos::interrupt_exit();
}

fn write_u32(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>, mut value: u32) {
    let mut digits = [0_u8; 10];
    let mut len = 0;
    loop {
        digits[len] = b'0' + (value % 10) as u8;
        len += 1;
        value /= 10;
        if value == 0 {
            break;
        }
    }
    for digit in digits[..len].iter().rev() {
        uart.write(core::slice::from_ref(digit));
    }
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(
        p.UART0,
        UartConfig {
            clock: UartClock::Boot,
            ..UartConfig::default()
        },
    );
    Watchdog::new(p.WDT).disable();
    uart.write(b"\r\nA3_STAGE_UART_OK\r\n");

    // SAFETY: ARENA is static, aligned, and no other owner can access it.
    unsafe { HEAP.init((*ARENA.0.get()).as_mut_ptr(), HEAP_SIZE).unwrap() };
    uart.write(b"A3_STAGE_HEAP_OK\r\n");

    let _timer = TimerAlarm0::new(p.TIMER);
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    hisi_rtos::start_with_port(
        hisi_rtos::Config {
            minimum_stack_size: NonZeroUsize::new(TASK_STACK_SIZE).unwrap(),
            scheduling: hisi_rtos::SchedulingPolicy::Priority,
            time_slice: NonZeroU32::new(5),
        },
        hisi_rtos::Resources {
            allocate,
            deallocate,
            monotonic_ms,
        },
        hisi_rtos::SchedulerPort {
            max_timer_delay: NonZeroU32::new(TimerAlarm0::MAX_DELAY_MS).unwrap(),
            arm_timer: TimerAlarm0::arm_millis,
            disarm_timer: TimerAlarm0::disarm,
            pend_reschedule: SoftwareInterrupt0::pend_interrupt,
        },
    )
    .unwrap();
    uart.write(b"A3_STAGE_RTOS_OK\r\n");

    unsafe { interrupt::enable_global() };
    hisi_rtos::request_reschedule();
    uart.write(b"A3_STAGE_IRQS_ON\r\n");

    for entry in [worker0, worker1] {
        hisi_rf_rtos_driver::spawn(
            entry,
            core::ptr::null_mut(),
            TaskConfig {
                stack_size: NonZeroUsize::new(TASK_STACK_SIZE).unwrap(),
                priority: 31,
            },
        )
        .unwrap();
    }
    uart.write(b"A3_STAGE_TASKS_OK\r\n");

    hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(500).unwrap()).unwrap();
    uart.write(b"A3_STAGE_SLEEP_RETURNED\r\n");

    let loops = critical_section::with(|cs| LOOPS.borrow(cs).get());
    let failures = critical_section::with(|cs| FAILURES.borrow(cs).get());
    let diagnostics = hisi_rtos::diagnostics();

    uart.write(b"\r\nA3 RTOS preemption diagnostic\r\nloops0=");
    write_u32(&uart, loops[0]);
    uart.write(b" loops1=");
    write_u32(&uart, loops[1]);
    uart.write(b" timer_irqs=");
    write_u32(&uart, diagnostics.timer_interrupts);
    uart.write(b" slice_preemptions=");
    write_u32(&uart, diagnostics.time_slice_preemptions);
    uart.write(b" software_irqs=");
    write_u32(&uart, diagnostics.software_interrupts);
    uart.write(b" fp_failures=");
    write_u32(&uart, failures);
    uart.write(b"\r\n");

    if loops[0] != 0
        && loops[1] != 0
        && diagnostics.timer_interrupts != 0
        && diagnostics.time_slice_preemptions != 0
        && diagnostics.software_interrupts != 0
        && failures == 0
    {
        uart.write(b"A3_RTOS_PREEMPTION_OK\r\n");
    } else {
        uart.write(b"A3_RTOS_PREEMPTION_FAIL\r\n");
    }

    loop {
        core::hint::spin_loop();
    }
}
