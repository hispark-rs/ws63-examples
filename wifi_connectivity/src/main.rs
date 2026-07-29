//! WS63 end-to-end connectivity through the public `hisi-rf` facade.
//!
//! The application uses one incremental radio runner for initialize, scan,
//! association, DHCP, repeated ICMP, and lease renewal. RF integration crates
//! remain transitive implementation details.

#![no_main]
#![no_std]

mod config;
mod network_runner;

use core::num::NonZeroU32;

use embassy_executor::{Executor, Spawner};
use embassy_time::{Duration, Timer, with_timeout};
use hisi_hal::Peripherals;
use hisi_hal::delay::Delay;
use hisi_hal::interrupt;
use hisi_hal::rf_power::RfPower;
use hisi_hal::software_interrupt::SoftwareInterrupt0;
use hisi_hal::time::Instant;
use hisi_hal::timer::TimerAlarm0;
use hisi_hal::uart::{Config as UartConfig, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
use hisi_rf::ws63::{
    IncrementalRadioParts, IncrementalRadioRunner, InstalledRadioStorage,
    RunnerDiagnosticsSnapshot, SelectedProfile, WaitDiagnosticsSnapshot, WifiDevice,
    declare_radio_storage,
};
use hisi_rf::{
    DiagnosticCode, IncrementalDriverEvent, Passphrase, ScanConfig, ScanResult, StationConfig,
    WifiController, WifiEvent,
};
#[cfg(feature = "wpa3")]
use hisi_rf::{SaePwe, Security};
use hisi_riscv_rt::entry;
use static_cell::StaticCell;

#[cfg(all(feature = "wpa2", feature = "wpa3"))]
compile_error!("select exactly one station security profile: wpa2 or wpa3");
#[cfg(not(any(feature = "wpa2", feature = "wpa3")))]
compile_error!("select exactly one station security profile: wpa2 or wpa3");

use config::{
    CONNECT_OPERATION_TIMEOUT, CONNECT_WAIT_DEADLINE, EVENT_WAIT_DEADLINE,
    INITIALIZE_WAIT_DEADLINE, RUNNER_BUDGET, SCAN_OPERATION_TIMEOUT, SCAN_RESULT_DEPTH,
    SCAN_WAIT_DEADLINE, TEST_PASSPHRASE, TEST_SSID,
};

type Uart0 = Uart<'static, hisi_hal::peripherals::Uart0<'static>>;

declare_radio_storage!(static RADIO_STORAGE);
static EXECUTOR: StaticCell<Executor> = StaticCell::new();
static UART: StaticCell<Uart0> = StaticCell::new();
static RADIO_PARTS: StaticCell<IncrementalRadioParts> = StaticCell::new();

#[entry]
fn main() -> ! {
    let p = Peripherals::take().expect("peripherals already taken");
    let uart = UART.init(Uart::new_uart0(
        p.UART0,
        UartConfig {
            clock: UartClock::Boot,
            ..UartConfig::default()
        },
    ));
    Watchdog::new(p.WDT).disable();
    uart.write(b"\r\nRFDBG_CONNECTIVITY_BEGIN facade=hisi-rf\r\n");

    let installed_storage = RADIO_STORAGE
        .install()
        .expect("install caller-owned radio storage");
    let mut delay = Delay::new();
    let rf_ready = RfPower::new(p.CMU, p.CLDO_CRG).enable(p.EFUSE, &mut delay);
    let (_cldo_crg, efuse) = rf_ready.into_parts();

    let _timer = TimerAlarm0::new(p.TIMER);
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    let runtime = hisi_rtos::start_with_port(
        hisi_rtos::PortedConfig {
            radio_task_policy: hisi_rtos::RunPolicy::Cooperative,
            max_scheduler_lock_duration: NonZeroU32::new(5_000).unwrap(),
            ..hisi_rtos::PortedConfig::default()
        },
        hisi_rtos::Resources {
            allocate: rtos_allocate,
            deallocate: rtos_deallocate,
            monotonic_ms,
        },
        hisi_rtos::SchedulerPort {
            max_timer_delay: NonZeroU32::new(TimerAlarm0::MAX_DELAY_MS)
                .expect("timer maximum delay must be non-zero"),
            arm_timer: TimerAlarm0::arm_millis,
            disarm_timer: TimerAlarm0::disarm,
            pend_reschedule: SoftwareInterrupt0::pend_interrupt,
            contract_violation: rtos_contract_violation,
        },
    )
    .expect("start ported runtime");
    let main_task = runtime.current_task().expect("adopted main task");
    runtime
        .set_task_run_policy(
            main_task,
            hisi_rtos::RunPolicy::Preemptive {
                time_slice: NonZeroU32::new(5).unwrap(),
            },
        )
        .expect("configure Embassy executor thread");

    unsafe { interrupt::enable_global() };
    hisi_rtos::request_reschedule();
    uart.write(b"RF1_IMAGE_OK\r\n");

    let (control_storage, radio_arena) = installed_storage.into_init_parts();
    let resources = hisi_rf::ws63::Resources::<SelectedProfile>::builder(efuse, radio_arena)
        .crypto(p.KM, p.SPACC, p.TRNG);
    #[cfg(feature = "wpa2")]
    let resources = resources.build();
    #[cfg(feature = "wpa3")]
    let resources = resources.pke(p.PKE).build();

    let controller = match hisi_rf::ws63::init(config::radio_config(), resources, control_storage) {
        Ok(controller) => controller,
        Err(error) => {
            write_diagnostic(uart, b"RF2_INIT_ERR:", error.diagnostic());
            halt()
        }
    };
    let parts = RADIO_PARTS.init(controller.split(RUNNER_BUDGET));
    start_executor(parts, uart)
}

#[inline(never)]
fn start_executor(parts: &'static mut IncrementalRadioParts, uart: &'static Uart0) -> ! {
    let IncrementalRadioParts { wifi, runner } = parts;
    let executor = EXECUTOR.init(Executor::new());
    executor.run(|spawner: Spawner| {
        spawner.spawn(radio_runner(runner, uart).unwrap());
        spawner.spawn(connectivity(&mut wifi.controller, &mut wifi.device, uart).unwrap());
    })
}

#[embassy_executor::task]
async fn radio_runner(runner: &'static mut IncrementalRadioRunner, uart: &'static Uart0) {
    loop {
        let ready = runner.wait_ready().await.expect("infallible WS63 wait");
        let started = monotonic_ms();
        let event = runner.run_once(ready).expect("incremental runner");
        uart.write(b"RFDBG_A5B_RUNNER_ELAPSED_MS value=0x");
        uart.write(&hex8(
            monotonic_ms().wrapping_sub(started).min(u32::MAX as u64) as u32,
        ));
        uart.write(b"\r\n");
        write_runner_event(uart, event);
    }
}

#[embassy_executor::task]
async fn connectivity(
    controller: &'static mut WifiController,
    device: &'static mut WifiDevice,
    uart: &'static Uart0,
) {
    let initialize_started = monotonic_ms();
    match with_timeout(INITIALIZE_WAIT_DEADLINE, controller.initialize()).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            write_controller_error(uart, b"RF2_INIT_ERR:", error);
            halt()
        }
        Err(_) => {
            uart.write(b"RF2_INIT_ERR:timeout\r\n");
            halt()
        }
    }
    uart.write(b"RFDBG_A5B_INITIALIZE_OK elapsed_ms=0x");
    uart.write(&hex8(
        monotonic_ms()
            .wrapping_sub(initialize_started)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b"\r\n");
    uart.write(b"RF2_INIT_OK ifname=hisi-rf\r\n");
    expect_event(uart, controller, ExpectedEvent::Initialized).await;

    let mut scan_results = [ScanResult::empty(); SCAN_RESULT_DEPTH];
    let mut retries = 0_u8;
    let scan_started = monotonic_ms();
    let outcome = loop {
        match with_timeout(
            SCAN_WAIT_DEADLINE,
            controller.scan(ScanConfig::new(SCAN_OPERATION_TIMEOUT), &mut scan_results),
        )
        .await
        {
            Ok(Ok(outcome)) => {
                if scan_results[..outcome.count]
                    .iter()
                    .any(|result| result.ssid.as_bytes() == TEST_SSID)
                {
                    break outcome;
                }
                if retries == 0 {
                    retries = 1;
                    Timer::after(Duration::from_millis(250)).await;
                    continue;
                }
                uart.write(b"RF5B_AP_NOT_FOUND\r\n");
                halt()
            }
            Ok(Err(error))
                if retries == 0
                    && error.diagnostic().code() == DiagnosticCode::OperationTimeout =>
            {
                retries = 1;
                Timer::after(Duration::from_millis(250)).await;
            }
            Ok(Err(error)) => {
                write_controller_error(uart, b"RF3_SCAN_ERR", error);
                halt()
            }
            Err(_) => {
                uart.write(b"RF3_SCAN_ERR reason=outer_timeout\r\n");
                halt()
            }
        }
    };
    uart.write(b"RFDBG_A5B_SCAN_OK elapsed_ms=0x");
    uart.write(&hex8(
        monotonic_ms()
            .wrapping_sub(scan_started)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b" count=0x");
    uart.write(&hex8(outcome.count.min(u32::MAX as usize) as u32));
    uart.write(b" truncated=0x");
    uart.write(&hex8(u32::from(outcome.truncated)));
    uart.write(b"\r\n");
    uart.write(b"RF3_SCAN_OK count=0x");
    uart.write(&hex8(outcome.count.min(u32::MAX as usize) as u32));
    uart.write(b" truncated=0x");
    uart.write(&hex8(u32::from(outcome.truncated)));
    uart.write(b"\r\n");
    expect_event(uart, controller, ExpectedEvent::ScanCompleted).await;

    let result = scan_results[..outcome.count]
        .iter()
        .find(|result| result.ssid.as_bytes() == TEST_SSID)
        .expect("scan result checked above");
    let Some(passphrase) = Passphrase::try_from_ascii(TEST_PASSPHRASE) else {
        uart.write(b"RF5B_CONNECT_ERR:invalid_credentials\r\n");
        halt()
    };
    #[cfg(feature = "wpa2")]
    let station = StationConfig::wpa2_personal(result, passphrase, CONNECT_OPERATION_TIMEOUT);
    #[cfg(feature = "wpa3")]
    let station =
        StationConfig::wpa3_personal(result, passphrase, SaePwe::Both, CONNECT_OPERATION_TIMEOUT);
    let Some(station) = station else {
        uart.write(b"RF5B_CONNECT_ERR:security_mismatch\r\n");
        halt()
    };

    uart.write(b"RF5B_CONNECT_BEGIN\r\n");
    let connect_started = monotonic_ms();
    match with_timeout(CONNECT_WAIT_DEADLINE, controller.connect(station)).await {
        Ok(Ok(_)) => {}
        Ok(Err(error)) => {
            write_controller_error(uart, b"RF5B_CONNECT_ERR:", error);
            write_a5b_evidence(uart, controller, device);
            halt()
        }
        Err(_) => {
            uart.write(b"RF5B_CONNECT_ERR:outer_timeout\r\n");
            halt()
        }
    }
    uart.write(b"RFDBG_A5B_CONNECT_OK elapsed_ms=0x");
    uart.write(&hex8(
        monotonic_ms()
            .wrapping_sub(connect_started)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b"\r\n");
    #[cfg(feature = "wpa2")]
    uart.write(b"W2D_WPA2_CONNECT_OK\r\n");
    #[cfg(feature = "wpa3")]
    {
        uart.write(b"W2E_WPA3_CONNECT_OK pmf=required\r\n");
        match result.security {
            Security::Wpa3Personal => uart.write(b"W2E_AP_SECURITY mode=pure-wpa3\r\n"),
            Security::Wpa2Wpa3PersonalTransition => {
                uart.write(b"W2E_AP_SECURITY mode=transition\r\n");
            }
            _ => {}
        }
    }
    expect_event(uart, controller, ExpectedEvent::Connected).await;
    write_a5b_evidence(uart, controller, device);
    network_runner::run(uart, device).await
}

enum ExpectedEvent {
    Initialized,
    ScanCompleted,
    Connected,
}

async fn expect_event(uart: &Uart0, controller: &mut WifiController, expected: ExpectedEvent) {
    let event = with_timeout(EVENT_WAIT_DEADLINE, controller.next_event())
        .await
        .unwrap_or_else(|_| {
            uart.write(b"RFDBG_A4_EVENT_ERR reason=timeout\r\n");
            halt()
        });
    let matches = match (expected, event) {
        (ExpectedEvent::Initialized, WifiEvent::Initialized) => {
            uart.write(b"A4_RADIO_EVENT kind=initialized\r\n");
            true
        }
        (ExpectedEvent::ScanCompleted, WifiEvent::ScanCompleted { .. }) => {
            uart.write(b"A4_RADIO_EVENT kind=scan-completed\r\n");
            true
        }
        (ExpectedEvent::Connected, WifiEvent::Connected(_)) => {
            uart.write(b"A4_RADIO_EVENT kind=connected\r\n");
            true
        }
        _ => false,
    };
    if !matches {
        uart.write(b"RFDBG_A4_EVENT_ERR reason=unexpected\r\n");
        halt()
    }
}

fn write_a5b_evidence(uart: &Uart0, controller: &WifiController, device: &WifiDevice) {
    let snapshot = hisi_rf::ws63::diagnostics(controller, device);
    let event = snapshot.events;
    uart.write(b"RFDBG_A5B_EVENT pending=0x");
    uart.write(&hex8(event.pending.min(u32::MAX as usize) as u32));
    uart.write(b" high_water=0x");
    uart.write(&hex8(event.high_water.min(u32::MAX as usize) as u32));
    uart.write(b" dropped=0x");
    uart.write(&hex8(event.dropped));
    uart.write(b"\r\n");

    let control = snapshot.control;
    uart.write(b"RFDBG_A5B_CONTROL pending=0x");
    uart.write(&hex8(
        control.command_queue_pending.min(u32::MAX as usize) as u32
    ));
    uart.write(b" high_water=0x");
    uart.write(&hex8(
        control.command_queue_high_water.min(u32::MAX as usize) as u32,
    ));
    uart.write(b"\r\n");

    match snapshot.runner {
        RunnerDiagnosticsSnapshot::Incremental(value) => write_runner_diagnostics(uart, value),
        RunnerDiagnosticsSnapshot::Blocking(_) => {
            uart.write(b"RFDBG_A5B_RUNNER_ERR reason=wrong_profile\r\n");
        }
    }
    write_wait_diagnostics(uart, snapshot.wait);

    let blocking = snapshot.blocking_calls;
    uart.write(b"RFDBG_A5B_BLOCKING init_calls=0x");
    uart.write(&hex8(blocking.initialize.calls));
    uart.write(b" init_max_ms=0x");
    uart.write(&hex8(blocking.initialize.max_elapsed_ms));
    uart.write(b" scan_calls=0x");
    uart.write(&hex8(blocking.scan.calls));
    uart.write(b" poll_calls=0x");
    uart.write(&hex8(blocking.poll.calls));
    uart.write(b" internal_sleep=0x");
    uart.write(&hex8(blocking.internal_sleep_calls));
    uart.write(b" supplicant_poll=0x");
    uart.write(&hex8(blocking.supplicant_poll_calls));
    uart.write(b"\r\n");

    let association = hisi_rf::ws63::association_timing_diagnostics();
    uart.write(b"RFDBG_A5B_CONNECT_ASSOC_IOCTL");
    for value in [
        association.first.calls,
        association.first.last_elapsed_ms,
        association.first.max_elapsed_ms,
        association.clear.calls,
        association.clear.last_elapsed_ms,
        association.clear.max_elapsed_ms,
        association.retry.calls,
        association.retry.last_elapsed_ms,
        association.retry.max_elapsed_ms,
        association.deauthenticate.calls,
        association.deauthenticate.last_elapsed_ms,
        association.deauthenticate.max_elapsed_ms,
    ] {
        uart.write(b" 0x");
        uart.write(&hex8(value));
    }
    uart.write(b"\r\nRFDBG_A5B_CONNECT_PROFILE_OK\r\n");
}

fn write_runner_diagnostics(uart: &Uart0, diagnostics: hisi_rf::IncrementalRunnerDiagnostics) {
    uart.write(b"RFDBG_A5B_RUNNER run=0x");
    uart.write(&hex8(diagnostics.run_once_calls));
    uart.write(b" waits=0x");
    uart.write(&hex8(diagnostics.wait_ready_calls));
    uart.write(b" wake=0x");
    uart.write(&hex8(diagnostics.wait_ready_completions));
    uart.write(b" immediate=0x");
    uart.write(&hex8(diagnostics.immediate_ready_completions));
    uart.write(b" operations=0x");
    uart.write(&hex8(diagnostics.operations_started));
    uart.write(b" completed=0x");
    uart.write(&hex8(diagnostics.operations_completed));
    uart.write(b" pending=0x");
    uart.write(&hex8(diagnostics.pending_polls));
    uart.write(b" exhausted=0x");
    uart.write(&hex8(diagnostics.budget_exhaustions));
    uart.write(b" errors=0x");
    uart.write(&hex8(
        diagnostics
            .driver_errors
            .saturating_add(diagnostics.protocol_errors)
            .saturating_add(diagnostics.wait_ready_errors),
    ));
    uart.write(b"\r\n");
}

fn write_wait_diagnostics(uart: &Uart0, diagnostics: WaitDiagnosticsSnapshot) {
    uart.write(b"RFDBG_A5B_WAIT backend=0x");
    uart.write(&hex8(diagnostics.backend_signals));
    uart.write(b" l2=0x");
    uart.write(&hex8(diagnostics.l2_rx_signals));
    uart.write(b" waker=0x");
    uart.write(&hex8(diagnostics.waker_notifications));
    uart.write(b" polls=0x");
    uart.write(&hex8(diagnostics.poll_calls));
    uart.write(b" pending=0x");
    uart.write(&hex8(diagnostics.pending_polls));
    uart.write(b" ready=0x");
    uart.write(&hex8(diagnostics.ready_polls));
    uart.write(b" timer=0x");
    uart.write(&hex8(diagnostics.timer_ready_polls));
    uart.write(b"\r\n");
}

fn write_runner_event(uart: &Uart0, event: IncrementalDriverEvent) {
    uart.write(b"RFDBG_A5B_RUNNER_EVENT kind=");
    match event {
        IncrementalDriverEvent::Idle => uart.write(b"idle"),
        IncrementalDriverEvent::Started { .. } => uart.write(b"started"),
        IncrementalDriverEvent::Waiting { .. } => uart.write(b"waiting"),
        IncrementalDriverEvent::Pending { .. } => uart.write(b"pending"),
        IncrementalDriverEvent::BudgetExhausted { .. } => uart.write(b"budget-exhausted"),
        IncrementalDriverEvent::CancelRequested { .. } => uart.write(b"cancel-requested"),
        IncrementalDriverEvent::Completed { .. } => uart.write(b"completed"),
        IncrementalDriverEvent::Cancelled { .. } => uart.write(b"cancelled"),
        IncrementalDriverEvent::Failed { .. } => uart.write(b"failed"),
    }
    uart.write(b"\r\n");
}

fn write_controller_error(uart: &Uart0, prefix: &[u8], error: hisi_rf::Error) {
    write_diagnostic(uart, prefix, error.diagnostic());
}

fn write_diagnostic(uart: &Uart0, prefix: &[u8], diagnostic: hisi_rf::Diagnostic) {
    uart.write(prefix);
    uart.write(b" code=");
    uart.write(diagnostic.code().as_str().as_bytes());
    uart.write(b" stage=");
    uart.write(diagnostic.stage().as_str().as_bytes());
    if let Some(code) = diagnostic.backend_code() {
        uart.write(b" backend=0x");
        uart.write(&hex8(code));
    }
    uart.write(b"\r\n");
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

unsafe fn rtos_allocate(size: usize) -> *mut u8 {
    unsafe { InstalledRadioStorage::allocate(size) }
}

unsafe fn rtos_deallocate(pointer: *mut u8) {
    unsafe { InstalledRadioStorage::deallocate(pointer) };
}

pub(crate) fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_violation: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}

pub(crate) fn halt() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

pub(crate) fn hex8(value: u32) -> [u8; 8] {
    let mut output = [0_u8; 8];
    for (index, digit) in output.iter_mut().enumerate() {
        let nibble = ((value >> ((7 - index) * 4)) & 0xf) as u8;
        *digit = if nibble < 10 {
            b'0' + nibble
        } else {
            b'a' + nibble - 10
        };
    }
    output
}

pub(crate) fn write_ipv4(uart: &Uart0, octets: [u8; 4]) {
    for (index, octet) in octets.iter().enumerate() {
        if index != 0 {
            uart.write(b".");
        }
        let hundreds = octet / 100;
        let tens = (octet % 100) / 10;
        let ones = octet % 10;
        if hundreds != 0 {
            uart.write(&[b'0' + hundreds]);
        }
        if hundreds != 0 || tens != 0 {
            uart.write(&[b'0' + tens]);
        }
        uart.write(&[b'0' + ones]);
    }
}
