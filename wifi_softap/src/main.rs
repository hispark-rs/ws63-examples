//! Fixed, non-production SoftAP for two-board WS63 HIL.

#![no_main]
#![no_std]

#[cfg(any(
    all(feature = "wpa2", feature = "wpa3"),
    not(any(feature = "wpa2", feature = "wpa3"))
))]
compile_error!("select exactly one SoftAP security profile: `wpa2` or `wpa3`");

#[path = "../../hil_wifi_config.rs"]
mod config;
mod network;

use core::num::NonZeroU32;

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
    AccessPointConfig, AccessPointResources, InstalledAccessPointStorage,
    declare_access_point_storage,
};
use hisi_riscv_rt::entry;

declare_access_point_storage!(static RADIO_STORAGE);

#[entry]
fn main() -> ! {
    let p = Peripherals::take().expect("peripherals already taken");
    let uart = Uart::new_uart0(
        p.UART0,
        UartConfig {
            clock: UartClock::Boot,
            ..UartConfig::default()
        },
    );
    Watchdog::new(p.WDT).disable();
    uart.write(b"\r\nRFDBG_SOFTAP_BEGIN\r\n");

    let installed = RADIO_STORAGE.install().expect("install SoftAP storage");
    let mut delay = Delay::new();
    let rf_ready = RfPower::new(p.CMU, p.CLDO_CRG).enable(p.EFUSE, &mut delay);
    let (_cldo_crg, efuse) = rf_ready.into_parts();

    let _timer = TimerAlarm0::new(p.TIMER);
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    let _runtime = hisi_rtos::start_with_port(
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
            max_timer_delay: NonZeroU32::new(TimerAlarm0::MAX_DELAY_MS).unwrap(),
            arm_timer: TimerAlarm0::arm_millis,
            disarm_timer: TimerAlarm0::disarm,
            pend_reschedule: SoftwareInterrupt0::pend_interrupt,
            contract_violation: rtos_contract_violation,
        },
    )
    .expect("start ported runtime");
    unsafe { interrupt::enable_global() };

    #[cfg(feature = "wpa2")]
    let resources = AccessPointResources::new(efuse, p.KM, p.SPACC, p.TRNG, installed);
    #[cfg(feature = "wpa3")]
    let resources = AccessPointResources::new(efuse, p.KM, p.SPACC, p.PKE, p.TRNG, installed);
    #[cfg(feature = "wpa2")]
    let config =
        AccessPointConfig::wpa2_personal(config::SSID, config::PASSPHRASE, config::CHANNEL);
    #[cfg(feature = "wpa3")]
    let config = AccessPointConfig::wpa3_sae(config::SSID, config::PASSPHRASE, config::CHANNEL);
    let mut access_point =
        hisi_rf::ws63::init_access_point(config, resources).expect("start native SoftAP");
    uart.write(b"RFDBG_SOFTAP_READY\r\n");
    let network_device = access_point
        .take_network_device()
        .expect("take SoftAP network device");
    network::run(access_point, network_device, &uart)
}

fn write_diagnostics(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    diagnostics: hisi_rf::ws63::AccessPointDiagnostics,
) {
    uart.write(b"RFDBG_SOFTAP_STATE event=");
    uart.write(&hex8(diagnostics.events));
    uart.write(b" last=");
    uart.write(&hex8(diagnostics.last_event as u32));
    uart.write(b" len=");
    uart.write(&hex8(diagnostics.last_event_length));
    uart.write(b" invalid=");
    uart.write(&hex8(diagnostics.invalid_events));
    uart.write(b" mgmt_q=");
    uart.write(&hex8(diagnostics.management_queued));
    uart.write(b" mgmt_drop=");
    uart.write(&hex8(diagnostics.management_dropped));
    uart.write(b" mgmt_feed=");
    uart.write(&hex8(diagnostics.management_fed));
    uart.write(b" mgmt_feed_err=");
    uart.write(&hex8(diagnostics.management_feed_errors));
    uart.write(b" sta_assoc=");
    uart.write(&hex8(diagnostics.stations_associated));
    uart.write(b" sta_disassoc=");
    uart.write(&hex8(diagnostics.stations_disassociated));
    uart.write(b" sta_err=");
    uart.write(&hex8(diagnostics.station_feed_errors));
    uart.write(b" mgmt_tx=");
    uart.write(&hex8(diagnostics.management_transmits));
    uart.write(b" mgmt_tx_status=");
    uart.write(&hex8(diagnostics.last_management_status as u32));
    uart.write(b" eapol_poll=");
    uart.write(&hex8(diagnostics.eapol_polls));
    uart.write(b" eapol_rx=");
    uart.write(&hex8(diagnostics.eapol_received));
    uart.write(b" eapol_feed=");
    uart.write(&hex8(diagnostics.eapol_fed));
    uart.write(b" eapol_err=");
    uart.write(&hex8(diagnostics.eapol_errors));
    uart.write(b" eapol_tx=");
    uart.write(&hex8(diagnostics.eapol_transmits));
    uart.write(b" eapol_tx_status=");
    uart.write(&hex8(diagnostics.last_eapol_status as u32));
    uart.write(b" key=");
    uart.write(&hex8(diagnostics.key_installs));
    uart.write(b" key_status=");
    uart.write(&hex8(diagnostics.last_key_status as u32));
    uart.write(b"\r\n");
}

fn hex8(value: u32) -> [u8; 8] {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = [0; 8];
    let mut index = 0;
    while index < output.len() {
        let shift = (output.len() - 1 - index) * 4;
        output[index] = HEX[((value >> shift) & 0xf) as usize];
        index += 1;
    }
    output
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
    unsafe {
        InstalledAccessPointStorage::<{ hisi_rf::ws63::ACCESS_POINT_ARENA_BYTES }>::allocate(size)
    }
}

unsafe fn rtos_deallocate(pointer: *mut u8) {
    unsafe {
        InstalledAccessPointStorage::<{ hisi_rf::ws63::ACCESS_POINT_ARENA_BYTES }>::deallocate(
            pointer,
        )
    };
}

fn monotonic_ms() -> u64 {
    Instant::now().raw() / 24_000
}

fn rtos_contract_violation(_violation: hisi_rtos::ContractViolation) -> ! {
    panic!("hisi-rtos scheduler contract violation")
}
