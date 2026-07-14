//! WS63 per-thread periodic budget enforcement HIL.
//!
//! A priority-1 worker never yields. Its 5 ms / 20 ms periodic CPU quota must
//! still let the cooperative priority-31 main task make progress. Repeated
//! exhaustion and replenishment prove that TIMER_INT0 and the common trap
//! epilogue enforce the `RunPolicy::Budgeted` upper bound on real silicon. This
//! test does not claim a minimum-service reservation.

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

const HEAP_SIZE: usize = 16 * 1024;
const STACK_SIZE: usize = 4 * 1024;

#[repr(C, align(16))]
struct Arena(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: the arena is registered once before interrupts are enabled and is
// thereafter reachable only through HEAP.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; HEAP_SIZE]));
static HEAP: CHeap = CHeap::empty();
static WORKER_LOOPS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

unsafe fn allocate(size: usize) -> *mut u8 {
    HEAP.allocate_zeroed(size, 16)
}

unsafe fn deallocate(pointer: *mut u8) {
    // SAFETY: hisi-rtos returns only live allocations created by HEAP.
    let _ = unsafe { HEAP.deallocate(pointer) };
}

fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}

extern "C" fn busy_worker(_: *mut c_void) -> *mut c_void {
    loop {
        critical_section::with(|cs| {
            let loops = WORKER_LOOPS.borrow(cs);
            loops.set(loops.get().saturating_add(1));
        });
    }
}

#[unsafe(no_mangle)]
extern "C" fn TIMER_INT0() {
    TimerAlarm0::clear_interrupt();
    hisi_rtos::interrupt_enter();
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
    // SAFETY: ARENA is static, aligned, and exclusively owned by HEAP.
    unsafe { HEAP.init((*ARENA.0.get()).as_mut_ptr(), HEAP_SIZE).unwrap() };

    let _timer = TimerAlarm0::new(p.TIMER);
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    let budget =
        hisi_rtos::BudgetSpec::try_new(NonZeroU32::new(5).unwrap(), NonZeroU32::new(20).unwrap())
            .unwrap();
    let _runtime = hisi_rtos::start_with_port(
        hisi_rtos::PortedConfig {
            minimum_stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
            radio_task_policy: hisi_rtos::RunPolicy::Budgeted(budget),
            max_scheduler_lock_duration: NonZeroU32::new(100).unwrap(),
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
            contract_violation: rtos_contract_violation,
        },
    )
    .unwrap();

    hisi_rf_rtos_driver::spawn(
        busy_worker,
        core::ptr::null_mut(),
        TaskConfig {
            stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
            priority: 1,
        },
    )
    .unwrap();
    unsafe { interrupt::enable_global() };
    // Budgeted keeps cooperative hand-off semantics until its budget expires.
    // Explicitly yield once so the worker starts; TIMER_INT0 must then throttle
    // it and restore this main task without cooperation from the worker.
    hisi_rf_rtos_driver::yield_now().unwrap();

    let start = monotonic_ms();
    let mut next_handoff = start.saturating_add(20);
    let mut main_loops = 0_u32;
    while monotonic_ms().saturating_sub(start) < 140 {
        main_loops = main_loops.saturating_add(1);
        let now = monotonic_ms();
        if now >= next_handoff {
            // Cooperative main explicitly hands off once per replenishment
            // period. The worker itself never yields; its budget must return
            // control to main after each handoff.
            hisi_rf_rtos_driver::yield_now().unwrap();
            next_handoff = next_handoff.saturating_add(20);
        }
        core::hint::spin_loop();
    }

    let worker_loops = critical_section::with(|cs| WORKER_LOOPS.borrow(cs).get());
    let diagnostics = hisi_rtos::diagnostics();
    uart.write(b"\r\nA3 budget diagnostic main_loops=");
    write_u32(&uart, main_loops);
    uart.write(b" worker_loops=");
    write_u32(&uart, worker_loops);
    uart.write(b" exhaustions=");
    write_u32(&uart, diagnostics.budget_exhaustions);
    uart.write(b" replenishments=");
    write_u32(&uart, diagnostics.budget_replenishments);
    uart.write(b" lock_overruns=");
    write_u32(&uart, diagnostics.budget_lock_overruns);
    uart.write(b"\r\n");

    if main_loops != 0
        && worker_loops != 0
        && diagnostics.budget_exhaustions >= 4
        && diagnostics.budget_replenishments >= 3
        && diagnostics.budget_lock_overruns == 0
    {
        uart.write(b"A3_RTOS_BUDGET_OK\r\n");
    } else {
        uart.write(b"A3_RTOS_BUDGET_FAIL\r\n");
    }

    loop {
        core::hint::spin_loop();
    }
}
