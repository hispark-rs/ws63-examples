//! Classic low/medium/high priority-inversion HIL for WS63.

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
use hisi_rf_rtos_driver::{MutexHandle, TaskConfig, WaitOutcome, WaitTimeout};
use hisi_riscv_rt::entry;

const HEAP_SIZE: usize = 48 * 1024;
const STACK_SIZE: usize = 4 * 1024;

#[repr(C, align(16))]
struct Arena(UnsafeCell<[u8; HEAP_SIZE]>);
// SAFETY: the allocator serializes access; initialization happens once before
// tasks and interrupts are enabled.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; HEAP_SIZE]));
static HEAP: CHeap = CHeap::empty();
static LOCK: Mutex<Cell<Option<MutexHandle>>> = Mutex::new(Cell::new(None));
static LOW_LOCKED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static LOW_RELEASED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static HIGH_ACQUIRED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static MEDIUM_LOOPS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

unsafe fn allocate(size: usize) -> *mut u8 {
    HEAP.allocate_zeroed(size, 16)
}

unsafe fn deallocate(pointer: *mut u8) {
    let _ = unsafe { HEAP.deallocate(pointer) };
}

fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn lock_handle() -> MutexHandle {
    critical_section::with(|cs| LOCK.borrow(cs).get().unwrap())
}

extern "C" fn low_task(_: *mut c_void) -> *mut c_void {
    let lock = lock_handle();
    assert_eq!(
        hisi_rf_rtos_driver::mutex_lock(lock, WaitTimeout::Forever).unwrap(),
        WaitOutcome::Acquired
    );
    critical_section::with(|cs| LOW_LOCKED.borrow(cs).set(true));
    hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(20).unwrap()).unwrap();
    hisi_rf_rtos_driver::mutex_unlock(lock).unwrap();
    critical_section::with(|cs| LOW_RELEASED.borrow(cs).set(true));
    core::ptr::null_mut()
}

extern "C" fn medium_task(_: *mut c_void) -> *mut c_void {
    while !critical_section::with(|cs| HIGH_ACQUIRED.borrow(cs).get()) {
        critical_section::with(|cs| {
            let loops = MEDIUM_LOOPS.borrow(cs);
            loops.set(loops.get().saturating_add(1));
        });
    }
    core::ptr::null_mut()
}

extern "C" fn high_task(_: *mut c_void) -> *mut c_void {
    let lock = lock_handle();
    assert_eq!(
        hisi_rf_rtos_driver::mutex_lock(lock, WaitTimeout::Forever).unwrap(),
        WaitOutcome::Acquired
    );
    critical_section::with(|cs| HIGH_ACQUIRED.borrow(cs).set(true));
    hisi_rf_rtos_driver::mutex_unlock(lock).unwrap();
    core::ptr::null_mut()
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
    unsafe { HEAP.init((*ARENA.0.get()).as_mut_ptr(), HEAP_SIZE).unwrap() };

    let _timer = TimerAlarm0::new(p.TIMER);
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    hisi_rtos::start_with_port(
        hisi_rtos::Config {
            minimum_stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
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
    let lock = hisi_rf_rtos_driver::mutex_create().unwrap();
    critical_section::with(|cs| LOCK.borrow(cs).set(Some(lock)));
    unsafe { interrupt::enable_global() };

    hisi_rf_rtos_driver::spawn(
        low_task,
        core::ptr::null_mut(),
        TaskConfig {
            stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
            priority: 20,
        },
    )
    .unwrap();
    hisi_rf_rtos_driver::yield_now().unwrap();
    assert!(critical_section::with(|cs| LOW_LOCKED.borrow(cs).get()));

    let tasks: [(hisi_rf_rtos_driver::TaskEntry, u8); 2] = [(medium_task, 10), (high_task, 2)];
    for (entry, priority) in tasks {
        hisi_rf_rtos_driver::spawn(
            entry,
            core::ptr::null_mut(),
            TaskConfig {
                stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
                priority,
            },
        )
        .unwrap();
    }
    hisi_rtos::request_reschedule();
    hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(200).unwrap()).unwrap();

    let low_released = critical_section::with(|cs| LOW_RELEASED.borrow(cs).get());
    let high_acquired = critical_section::with(|cs| HIGH_ACQUIRED.borrow(cs).get());
    let medium_loops = critical_section::with(|cs| MEDIUM_LOOPS.borrow(cs).get());
    let inherited = hisi_rtos::diagnostics().priority_inheritances;
    if low_released && high_acquired && medium_loops != 0 && inherited != 0 {
        uart.write(b"\r\nA3_PRIORITY_INHERITANCE_OK\r\n");
    } else {
        uart.write(b"\r\nA3_PRIORITY_INHERITANCE_FAIL\r\n");
    }
    loop {
        core::hint::spin_loop();
    }
}
