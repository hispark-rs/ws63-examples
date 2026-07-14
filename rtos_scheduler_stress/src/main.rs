//! Timed-wait and nested IRQ-bracket scheduler HIL for WS63.

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
use hisi_rf_rtos_driver::{SemaphoreHandle, TaskConfig, WaitOutcome, WaitTimeout};
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
static TIMEOUT_SEM: Mutex<Cell<Option<SemaphoreHandle>>> = Mutex::new(Cell::new(None));
static IRQ_SEM: Mutex<Cell<Option<SemaphoreHandle>>> = Mutex::new(Cell::new(None));
static IRQ_WAKE_ARMED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static IRQ_WAKE_POSTED: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static IN_HANDLER: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static RAN_IN_HANDLER: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static TIMEOUT_OK: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));
static IRQ_WAKE_OK: Mutex<Cell<bool>> = Mutex::new(Cell::new(false));

unsafe fn allocate(size: usize) -> *mut u8 {
    HEAP.allocate_zeroed(size, 16)
}

unsafe fn deallocate(pointer: *mut u8) {
    let _ = unsafe { HEAP.deallocate(pointer) };
}

fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}

fn semaphore(slot: &Mutex<Cell<Option<SemaphoreHandle>>>) -> SemaphoreHandle {
    critical_section::with(|cs| slot.borrow(cs).get().unwrap())
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

extern "C" fn timeout_task(_: *mut c_void) -> *mut c_void {
    let outcome =
        hisi_rf_rtos_driver::semaphore_down(semaphore(&TIMEOUT_SEM), WaitTimeout::from_millis(15))
            .unwrap();
    critical_section::with(|cs| TIMEOUT_OK.borrow(cs).set(outcome == WaitOutcome::TimedOut));
    core::ptr::null_mut()
}

extern "C" fn irq_woken_task(_: *mut c_void) -> *mut c_void {
    let outcome =
        hisi_rf_rtos_driver::semaphore_down(semaphore(&IRQ_SEM), WaitTimeout::Forever).unwrap();
    critical_section::with(|cs| {
        if IN_HANDLER.borrow(cs).get() {
            RAN_IN_HANDLER.borrow(cs).set(true);
        }
        IRQ_WAKE_OK.borrow(cs).set(outcome == WaitOutcome::Acquired);
    });
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
    critical_section::with(|cs| IN_HANDLER.borrow(cs).set(true));
    hisi_rtos::on_software_interrupt();
    let post = critical_section::with(|cs| {
        IRQ_WAKE_ARMED.borrow(cs).get() && !IRQ_WAKE_POSTED.borrow(cs).replace(true)
    });
    if post {
        // Exercise nested runtime/adapter IRQ bracketing. The hardware trap
        // keeps MIE disabled; this does not pretend WS63 has nested IRQ stacks.
        hisi_rtos::interrupt_enter();
        hisi_rf_rtos_driver::semaphore_up(semaphore(&IRQ_SEM)).unwrap();
        hisi_rtos::interrupt_exit();
    }

    critical_section::with(|cs| IN_HANDLER.borrow(cs).set(false));
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
    let _runtime = hisi_rtos::start_with_port(
        hisi_rtos::PortedConfig {
            minimum_stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
            radio_task_policy: hisi_rtos::RunPolicy::Preemptive {
                time_slice: NonZeroU32::new(5).unwrap(),
            },
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

    let timeout_sem = hisi_rf_rtos_driver::semaphore_create(0).unwrap();
    let irq_sem = hisi_rf_rtos_driver::semaphore_create(0).unwrap();
    critical_section::with(|cs| {
        TIMEOUT_SEM.borrow(cs).set(Some(timeout_sem));
        IRQ_SEM.borrow(cs).set(Some(irq_sem));
    });
    unsafe { interrupt::enable_global() };

    for (entry, priority) in [
        (timeout_task as hisi_rf_rtos_driver::TaskEntry, 2),
        (irq_woken_task as hisi_rf_rtos_driver::TaskEntry, 3),
    ] {
        hisi_rf_rtos_driver::spawn(
            entry,
            core::ptr::null_mut(),
            TaskConfig {
                stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
                priority,
            },
        )
        .unwrap();
        hisi_rf_rtos_driver::yield_now().unwrap();
    }

    critical_section::with(|cs| IRQ_WAKE_ARMED.borrow(cs).set(true));
    SoftwareInterrupt0::pend_interrupt();
    hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(100).unwrap()).unwrap();

    let (timeout_ok, irq_wake_ok, ran_in_handler, posted) = critical_section::with(|cs| {
        (
            TIMEOUT_OK.borrow(cs).get(),
            IRQ_WAKE_OK.borrow(cs).get(),
            RAN_IN_HANDLER.borrow(cs).get(),
            IRQ_WAKE_POSTED.borrow(cs).get(),
        )
    });
    let diagnostics = hisi_rtos::diagnostics();
    uart.write(b"\r\nA3 scheduler stress diagnostic\r\ntimeout_ok=");
    write_u32(&uart, timeout_ok as u32);
    uart.write(b" irq_wake_ok=");
    write_u32(&uart, irq_wake_ok as u32);
    uart.write(b" ran_in_handler=");
    write_u32(&uart, ran_in_handler as u32);
    uart.write(b" posted=");
    write_u32(&uart, posted as u32);
    uart.write(b" timeout_count=");
    write_u32(&uart, diagnostics.semaphore_timeouts);
    uart.write(b" wake_count=");
    write_u32(&uart, diagnostics.semaphore_wakes);
    uart.write(b" software_irqs=");
    write_u32(&uart, diagnostics.software_interrupts);
    uart.write(b"\r\n");
    if timeout_ok
        && irq_wake_ok
        && !ran_in_handler
        && posted
        && diagnostics.semaphore_timeouts != 0
        && diagnostics.semaphore_wakes != 0
    {
        uart.write(b"\r\nA3_SCHEDULER_STRESS_OK\r\n");
    } else {
        uart.write(b"\r\nA3_SCHEDULER_STRESS_FAIL\r\n");
    }
    loop {
        core::hint::spin_loop();
    }
}
