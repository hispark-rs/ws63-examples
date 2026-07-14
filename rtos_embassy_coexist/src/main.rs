//! Embassy executor and native RTOS task sharing one WS63 scheduler timer.

#![no_std]
#![no_main]

use core::cell::{Cell, UnsafeCell};
use core::ffi::c_void;
use core::num::{NonZeroU32, NonZeroUsize};

use critical_section::Mutex;
use embassy_executor::{Executor, Spawner};
use embassy_time::Timer;
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
use static_cell::StaticCell;

const HEAP_SIZE: usize = 48 * 1024;
const STACK_SIZE: usize = 4 * 1024;

#[repr(C, align(16))]
struct Arena(UnsafeCell<[u8; HEAP_SIZE]>);

// SAFETY: initialization happens once before interrupts and spawned tasks.
unsafe impl Sync for Arena {}

static ARENA: Arena = Arena(UnsafeCell::new([0; HEAP_SIZE]));
static HEAP: CHeap = CHeap::empty();
static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static NATIVE_TICKS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));
static EMBASSY_TICKS: Mutex<Cell<u32>> = Mutex::new(Cell::new(0));

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

extern "C" fn native_worker(_: *mut c_void) -> *mut c_void {
    loop {
        critical_section::with(|cs| {
            NATIVE_TICKS
                .borrow(cs)
                .set(NATIVE_TICKS.borrow(cs).get().saturating_add(1));
        });
        hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(7).unwrap()).unwrap();
    }
}

#[embassy_executor::task]
async fn embassy_worker() {
    loop {
        Timer::after_millis(11).await;
        critical_section::with(|cs| {
            EMBASSY_TICKS
                .borrow(cs)
                .set(EMBASSY_TICKS.borrow(cs).get().saturating_add(1));
        });
    }
}

#[embassy_executor::task]
async fn reporter(uart: Uart<'static, hisi_hal::peripherals::Uart0<'static>>) {
    Timer::after_millis(120).await;
    let (native, embassy) = critical_section::with(|cs| {
        (
            NATIVE_TICKS.borrow(cs).get(),
            EMBASSY_TICKS.borrow(cs).get(),
        )
    });
    let diagnostics = hisi_rtos::diagnostics();

    uart.write(b"\r\nA3 RTOS Embassy coexist diagnostic\r\nnative_ticks=");
    write_u32(&uart, native);
    uart.write(b" embassy_ticks=");
    write_u32(&uart, embassy);
    uart.write(b" timer_irqs=");
    write_u32(&uart, diagnostics.timer_interrupts);
    uart.write(b" context_switches=");
    write_u32(&uart, diagnostics.context_switches);
    uart.write(b"\r\n");

    if native >= 8 && embassy >= 6 && diagnostics.timer_interrupts != 0 {
        uart.write(b"\r\nA3_RTOS_EMBASSY_COEXIST_OK\r\n");
    } else {
        uart.write(b"\r\nA3_RTOS_EMBASSY_COEXIST_FAIL\r\n");
    }

    loop {
        Timer::after_millis(1_000).await;
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
    let runtime = hisi_rtos::start_with_port(
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

    hisi_rf_rtos_driver::spawn(
        native_worker,
        core::ptr::null_mut(),
        TaskConfig {
            stack_size: NonZeroUsize::new(STACK_SIZE).unwrap(),
            priority: 3,
        },
    )
    .unwrap();

    // The adopted main thread hosts Embassy's executor. It does not call the
    // RTOS yield API while polling futures, so explicitly opt this thread into
    // preemption; Config::radio_task_policy applies only to spawned workers.
    let main_task = hisi_rf_rtos_driver::current_task().unwrap();
    runtime
        .set_task_run_policy(
            main_task,
            hisi_rtos::RunPolicy::Preemptive {
                time_slice: NonZeroU32::new(5).unwrap(),
            },
        )
        .unwrap();

    unsafe { interrupt::enable_global() };
    hisi_rtos::request_reschedule();
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(embassy_worker().unwrap());
        spawner.spawn(reporter(uart).unwrap());
    });
}
