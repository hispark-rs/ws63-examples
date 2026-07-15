//! WS63 RF init smoke.
//!
//! This is the first real-silicon RF milestone binary. By default it is an RF1
//! image smoke: it proves the runtime image, `.wifi_pkt_ram`, panic path, and
//! RF porting crate fit together. With `--features full-init`, it pulls in the
//! vendor Wi-Fi init closure and calls `uapi_wifi_init`. Build that path through
//! `rf-build-full-init-lld-layout-patch.sh`: stock `rust-lld` owns the final
//! layout while the guarded post-link lane resolves HiSilicon custom
//! relocations and verifies every patched section against that layout.

#![no_std]
#![no_main]

#[cfg(feature = "rf-vendor-log")]
use core::cell::RefCell;

#[cfg(all(feature = "wpa", feature = "wpa3"))]
compile_error!("select exactly one station security profile: wpa or wpa3");
#[cfg(all(feature = "upstream-supplicant", feature = "personal"))]
compile_error!("upstream-supplicant and the vendor personal profiles are mutually exclusive");
#[cfg(all(feature = "personal", not(any(feature = "wpa", feature = "wpa3"))))]
compile_error!("the internal personal feature requires either wpa or wpa3");

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
mod network_runner;

use hisi_hal::Peripherals;
use hisi_hal::delay::Delay;
#[cfg(feature = "full-init")]
use hisi_hal::interrupt;
use hisi_hal::rf_power::{FactoryXoTrim, RfPower};
#[cfg(feature = "full-init")]
use hisi_hal::software_interrupt::SoftwareInterrupt0;
use hisi_hal::system::{ResetReason, System};
#[cfg(feature = "full-init")]
use hisi_hal::timer::TimerAlarm0;
use hisi_hal::uart::{Config, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
use hisi_riscv_rt::entry;
#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
use static_cell::StaticCell;
#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
use ws63_rf_rs::hisi_rf_backend::{Ws63RadioState, Ws63WifiBackend};
#[cfg(feature = "full-init")]
use ws63_rf_rs::wifi::MAX_SCAN_RESULTS;
#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
use ws63_rf_rs::wifi::{Error as WifiError, OpenNetwork, ScanResult, Wifi as ActiveWifi};

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
const TEST_SSID: &[u8] = b"HUAWEI-HLJ";
#[cfg(feature = "personal")]
const TEST_SSID: &[u8] = match option_env!("WS63_WIFI_SSID") {
    Some(value) => value.as_bytes(),
    None => b"HUAWEI-HLJ_Guest",
};
#[cfg(feature = "personal")]
const TEST_PASSPHRASE: &[u8] = match option_env!("WS63_WIFI_PASSPHRASE") {
    Some(value) => value.as_bytes(),
    None => b"",
};
#[cfg(feature = "upstream-supplicant")]
const TEST_SSID: &[u8] = match option_env!("WS63_WIFI_SSID") {
    Some(value) => value.as_bytes(),
    None => b"HUAWEI-HLJ_Guest",
};
#[cfg(feature = "upstream-supplicant")]
const TEST_PASSPHRASE: &[u8] = match option_env!("WS63_WIFI_PASSPHRASE") {
    Some(value) => value.as_bytes(),
    None => b"testtest",
};

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
const RADIO_EVENT_DEPTH: usize = 8;
#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
type Ws63RadioRunner = hisi_rf::RadioRunner<Ws63WifiBackend<'static>, RADIO_EVENT_DEPTH>;
#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
static RADIO_STATE: Ws63RadioState<RADIO_EVENT_DEPTH> = Ws63RadioState::new();
#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
static RADIO_RUNNER: StaticCell<Ws63RadioRunner> = StaticCell::new();

#[cfg(feature = "rf-watchpoint-gate")]
#[unsafe(no_mangle)]
static mut __WS63_RF_SCAN_DEBUG_GATE: u32 = 0;

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

#[cfg(feature = "full-init")]
fn hex16(n: u64) -> [u8; 16] {
    let mut buf = [0u8; 16];
    let mut i = 0;
    while i < 16 {
        let nib = (n >> ((15 - i) * 4)) & 0xf;
        buf[i] = if nib < 10 {
            b'0' + nib as u8
        } else {
            b'a' + (nib - 10) as u8
        };
        i += 1;
    }
    buf
}

#[cfg(feature = "full-init")]
fn rtos_contract_violation(violation: hisi_rtos::ContractViolation) -> ! {
    match violation {
        hisi_rtos::ContractViolation::SchedulerLockOverrun {
            task_slot,
            held_ms,
            limit_ms,
        } => {
            rf_log_uart0(b"RFDBG_RTOS_LOCK_OVERRUN task=0x");
            rf_log_uart0(&hex8(task_slot as u32));
            rf_log_uart0(b" held_ms=0x");
            rf_log_uart0(&hex16(held_ms));
            rf_log_uart0(b" limit_ms=0x");
            rf_log_uart0(&hex8(limit_ms));
            rf_log_uart0(b"\r\n");
        }
    }
    panic!("hisi-rtos scheduler contract violation")
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(
        p.UART0,
        Config {
            clock: UartClock::Boot,
            ..Config::default()
        },
    );

    uart.write(b"RFDBG_RF_POWER_BEGIN\r\n");
    let mut delay = Delay::new();
    let rf_ready = RfPower::new(p.CMU, p.CLDO_CRG).enable(p.EFUSE, &mut delay);
    match rf_ready.factory_xo_trim() {
        FactoryXoTrim::Default => uart.write(b"RFDBG_XO_TRIM_DEFAULT\r\n"),
        FactoryXoTrim::Applied {
            group,
            fine,
            coarse,
        } => {
            uart.write(b"RFDBG_XO_TRIM group=0x");
            uart.write(&hex8(group as u32));
            uart.write(b" fine=0x");
            uart.write(&hex8(fine as u32));
            uart.write(b" coarse=0x");
            uart.write(&hex8(coarse as u32));
            uart.write(b"\r\n");
        }
    }
    let (cldo_crg, efuse) = rf_ready.into_parts();
    uart.write(b"RFDBG_RF_POWER_OK\r\n");

    let system = System::new(p.SYS_CTL0, p.GLB_CTL_M, cldo_crg);
    uart.write(b"RFDBG_RESET_REASON=");
    uart.write(match system.reset_reason() {
        ResetReason::PowerOn => b"power-on\r\n",
        ResetReason::ExternalPin => b"external-pin\r\n",
        ResetReason::Watchdog => b"watchdog\r\n",
        ResetReason::Software => b"software\r\n",
        ResetReason::BrownOut => b"brown-out\r\n",
        ResetReason::Unknown => b"unknown\r\n",
    });

    // Flashboot deliberately arms a 7-second reset watchdog and transfers
    // control after one final kick. The vendor application reconfigures it in
    // `hw_init`; this bare-metal smoke must take ownership before the much
    // longer Wi-Fi initialization path starts.
    let mut boot_watchdog = Watchdog::new(p.WDT);
    boot_watchdog.disable();
    uart.write(b"RFDBG_BOOT_WDT_DISABLED\r\n");

    #[cfg(feature = "rf-vendor-log")]
    ws63_rf_rs::set_log_sink(rf_log_buffered);
    #[cfg(feature = "full-init")]
    let _timer_alarm = TimerAlarm0::new(p.TIMER);
    #[cfg(feature = "full-init")]
    let _software_interrupt = SoftwareInterrupt0::new(p.SYS_CTL1);
    #[cfg(feature = "full-init")]
    let _runtime = hisi_rtos::start_with_port(
        hisi_rtos::PortedConfig {
            radio_task_policy: hisi_rtos::RunPolicy::Cooperative,
            ..hisi_rtos::PortedConfig::default()
        },
        hisi_rtos::Resources {
            allocate: rtos_allocate,
            deallocate: rtos_deallocate,
            monotonic_ms: ws63_rf_rs::uapi::monotonic_ms,
        },
        hisi_rtos::SchedulerPort {
            max_timer_delay: core::num::NonZeroU32::new(TimerAlarm0::MAX_DELAY_MS).unwrap(),
            arm_timer: TimerAlarm0::arm_millis,
            disarm_timer: TimerAlarm0::disarm,
            pend_reschedule: SoftwareInterrupt0::pend_interrupt,
            contract_violation: rtos_contract_violation,
        },
    )
    .expect("start radio runtime");

    // `start_with_port` installs the timer/SWI delivery mechanism but leaves
    // the global machine-interrupt policy to the application. Ported yields
    // are invalid until MIE is enabled because their handoff completes through
    // the software-interrupt trap path.
    #[cfg(feature = "full-init")]
    unsafe {
        interrupt::enable_global()
    };
    #[cfg(feature = "full-init")]
    hisi_rtos::request_reschedule();

    uart.write(b"\r\nRF1_IMAGE_OK\r\n");
    run_wifi_smoke(&uart, efuse);

    loop {
        core::hint::spin_loop();
    }
}

#[cfg(feature = "full-init")]
#[unsafe(no_mangle)]
extern "C" fn TIMER_INT0() {
    TimerAlarm0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    hisi_rtos::on_timer_interrupt();
    hisi_rtos::interrupt_exit();
}

#[cfg(feature = "full-init")]
#[unsafe(no_mangle)]
extern "C" fn SOFT_INT0() {
    SoftwareInterrupt0::clear_interrupt();
    hisi_rtos::interrupt_enter();
    hisi_rtos::on_software_interrupt();
    hisi_rtos::interrupt_exit();
}

#[cfg(feature = "full-init")]
unsafe fn rtos_allocate(size: usize) -> *mut u8 {
    ws63_rf_rs::alloc::osal_kmalloc(size).cast()
}

#[cfg(feature = "full-init")]
unsafe fn rtos_deallocate(pointer: *mut u8) {
    ws63_rf_rs::alloc::osal_kfree(pointer.cast());
}

#[cfg(feature = "full-init")]
fn rf_log_uart0(bytes: &[u8]) {
    const DATA: *mut u16 = 0x4401_0004 as *mut u16;
    const ST: *const u16 = 0x4401_0044 as *const u16;
    const TX_FULL: u16 = 1 << 0;
    const TX_EMPTY: u16 = 1 << 1;

    for &byte in bytes {
        unsafe {
            while core::ptr::read_volatile(ST) & TX_FULL != 0 {
                core::hint::spin_loop();
            }
            core::ptr::write_volatile(DATA, byte as u16);
            while core::ptr::read_volatile(ST) & TX_EMPTY == 0 {
                core::hint::spin_loop();
            }
        }
    }
}

#[cfg(feature = "rf-vendor-log")]
const VENDOR_LOG_CAPACITY: usize = 16 * 1024;

#[cfg(feature = "rf-vendor-log")]
struct VendorLogBuffer {
    bytes: [u8; VENDOR_LOG_CAPACITY],
    start: usize,
    len: usize,
    dropped: usize,
}

#[cfg(feature = "rf-vendor-log")]
impl VendorLogBuffer {
    const fn new() -> Self {
        Self {
            bytes: [0; VENDOR_LOG_CAPACITY],
            start: 0,
            len: 0,
            dropped: 0,
        }
    }

    fn push(&mut self, bytes: &[u8]) {
        let bytes = if bytes.len() >= VENDOR_LOG_CAPACITY {
            self.dropped = self
                .dropped
                .saturating_add(self.len)
                .saturating_add(bytes.len() - VENDOR_LOG_CAPACITY);
            self.start = 0;
            self.len = 0;
            &bytes[bytes.len() - VENDOR_LOG_CAPACITY..]
        } else {
            bytes
        };
        let overflow = self
            .len
            .saturating_add(bytes.len())
            .saturating_sub(VENDOR_LOG_CAPACITY);
        if overflow != 0 {
            self.start = (self.start + overflow) % VENDOR_LOG_CAPACITY;
            self.len -= overflow;
            self.dropped = self.dropped.saturating_add(overflow);
        }
        let end = (self.start + self.len) % VENDOR_LOG_CAPACITY;
        let first = bytes.len().min(VENDOR_LOG_CAPACITY - end);
        self.bytes[end..end + first].copy_from_slice(&bytes[..first]);
        self.bytes[..bytes.len() - first].copy_from_slice(&bytes[first..]);
        self.len += bytes.len();
    }

    fn copy_from(&self, offset: usize, output: &mut [u8]) -> usize {
        let count = (self.len - offset).min(output.len());
        let source = (self.start + offset) % VENDOR_LOG_CAPACITY;
        let first = count.min(VENDOR_LOG_CAPACITY - source);
        output[..first].copy_from_slice(&self.bytes[source..source + first]);
        output[first..count].copy_from_slice(&self.bytes[..count - first]);
        count
    }
}

#[cfg(feature = "rf-vendor-log")]
static VENDOR_LOG: critical_section::Mutex<RefCell<VendorLogBuffer>> =
    critical_section::Mutex::new(RefCell::new(VendorLogBuffer::new()));

#[cfg(feature = "rf-vendor-log")]
fn rf_log_buffered(bytes: &[u8]) {
    critical_section::with(|cs| VENDOR_LOG.borrow(cs).borrow_mut().push(bytes));
}

#[cfg(feature = "rf-vendor-log")]
fn rf_log_drop(_: &[u8]) {}

#[cfg(feature = "rf-vendor-log")]
fn flush_vendor_log(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    // Stop producers before reading the fixed buffer. Vendor callbacks can run
    // from several worker contexts, so streaming directly to a 115200-baud UART
    // would extend scheduler-lock regions and change the behavior being traced.
    ws63_rf_rs::set_log_sink(rf_log_drop);
    uart.write(b"RFDBG_VENDOR_LOG_BEGIN\r\n");
    let mut offset = 0;
    let mut chunk = [0_u8; 128];
    loop {
        let (count, dropped) = critical_section::with(|cs| {
            let log = VENDOR_LOG.borrow(cs).borrow();
            let count = log.copy_from(offset, &mut chunk);
            (count, log.dropped)
        });
        if count == 0 {
            uart.write(b"\r\nRFDBG_VENDOR_LOG_END dropped=0x");
            uart.write(&hex8(dropped.min(u32::MAX as usize) as u32));
            uart.write(b"\r\n");
            break;
        }
        uart.write(&chunk[..count]);
        offset += count;
    }
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
struct VendorLogGuard<'a, 'd> {
    _uart: &'a Uart<'d, hisi_hal::peripherals::Uart0<'d>>,
    flushed: bool,
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
impl<'a, 'd> VendorLogGuard<'a, 'd> {
    fn new(uart: &'a Uart<'d, hisi_hal::peripherals::Uart0<'d>>) -> Self {
        Self {
            _uart: uart,
            flushed: false,
        }
    }

    fn flush(&mut self) {
        if self.flushed {
            return;
        }
        #[cfg(feature = "rf-vendor-log")]
        flush_vendor_log(self._uart);
        self.flushed = true;
    }
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
impl Drop for VendorLogGuard<'_, '_> {
    fn drop(&mut self) {
        self.flush();
    }
}

#[cfg(all(feature = "full-init", target_arch = "riscv32"))]
#[unsafe(no_mangle)]
extern "C" fn __ws63_rf_exception_diag(frame: *const u32) -> ! {
    let (mcause, mepc, mtval): (u32, u32, u32);
    // SAFETY: the runtime invokes this handler from machine mode after saving
    // the interrupted context; these read-only CSR accesses preserve it.
    unsafe {
        core::arch::asm!("csrr {value}, mcause", value = out(reg) mcause, options(nomem, nostack));
        core::arch::asm!("csrr {value}, mepc", value = out(reg) mepc, options(nomem, nostack));
        core::arch::asm!("csrr {value}, mtval", value = out(reg) mtval, options(nomem, nostack));
    }
    rf_log_uart0(b"RFDBG_EXCEPTION cause=0x");
    rf_log_uart0(&hex8(mcause));
    rf_log_uart0(b" epc=0x");
    rf_log_uart0(&hex8(mepc));
    rf_log_uart0(b" tval=0x");
    rf_log_uart0(&hex8(mtval));
    if !frame.is_null() {
        // SAFETY: WS63 trap_entry passes the live saved-register frame in a0.
        // The integer prefix offsets mirror startup.S save_all/restore_all;
        // floating-point state is appended without changing this ABI.
        let (ra, a0, a1, a2, a3, a4, a5, s0, s1, s2) = unsafe {
            (
                core::ptr::read_volatile(frame.byte_add(0x8c)),
                core::ptr::read_volatile(frame.byte_add(0x7c)),
                core::ptr::read_volatile(frame.byte_add(0x78)),
                core::ptr::read_volatile(frame.byte_add(0x74)),
                core::ptr::read_volatile(frame.byte_add(0x70)),
                core::ptr::read_volatile(frame.byte_add(0x6c)),
                core::ptr::read_volatile(frame.byte_add(0x68)),
                core::ptr::read_volatile(frame.byte_add(0x4c)),
                core::ptr::read_volatile(frame.byte_add(0x48)),
                core::ptr::read_volatile(frame.byte_add(0x44)),
            )
        };
        rf_log_uart0(b" frame=0x");
        rf_log_uart0(&hex8(frame as u32));
        rf_log_uart0(b" ra=0x");
        rf_log_uart0(&hex8(ra));
        rf_log_uart0(b" a0=0x");
        rf_log_uart0(&hex8(a0));
        rf_log_uart0(b" a1=0x");
        rf_log_uart0(&hex8(a1));
        rf_log_uart0(b" a2=0x");
        rf_log_uart0(&hex8(a2));
        rf_log_uart0(b" a3=0x");
        rf_log_uart0(&hex8(a3));
        rf_log_uart0(b" a4=0x");
        rf_log_uart0(&hex8(a4));
        rf_log_uart0(b" a5=0x");
        rf_log_uart0(&hex8(a5));
        rf_log_uart0(b" s0=0x");
        rf_log_uart0(&hex8(s0));
        rf_log_uart0(b" s1=0x");
        rf_log_uart0(&hex8(s1));
        rf_log_uart0(b" s2=0x");
        rf_log_uart0(&hex8(s2));
    }
    let mut frees = [ws63_rf_rs::alloc::FreeTraceRecord::default(); 16];
    let free_count = ws63_rf_rs::alloc::free_trace_snapshot(&mut frees);
    for record in frees[..free_count]
        .iter()
        .filter(|record| record.sequence != 0)
    {
        rf_log_uart0(b"\r\nRFDBG_FREE seq=0x");
        rf_log_uart0(&hex8(record.sequence));
        rf_log_uart0(b" ptr=0x");
        rf_log_uart0(&hex8(record.pointer as u32));
        rf_log_uart0(b" ra=0x");
        rf_log_uart0(&hex8(record.caller as u32));
    }
    let mut allocations = [ws63_rf_rs::alloc::AllocationTraceRecord::default(); 16];
    let allocation_count = ws63_rf_rs::alloc::allocation_trace_snapshot(&mut allocations);
    for record in allocations[..allocation_count]
        .iter()
        .filter(|record| record.sequence != 0)
    {
        rf_log_uart0(b"\r\nRFDBG_ALLOC seq=0x");
        rf_log_uart0(&hex8(record.sequence));
        rf_log_uart0(b" ptr=0x");
        rf_log_uart0(&hex8(record.pointer as u32));
        rf_log_uart0(b" size=0x");
        rf_log_uart0(&hex8(record.size as u32));
        rf_log_uart0(b" ra=0x");
        rf_log_uart0(&hex8(record.caller as u32));
    }
    rf_log_uart0(b"\r\n");
    dump_rtos_task_metrics();
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(feature = "full-init"))]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    _efuse: hisi_hal::peripherals::Efuse<'_>,
) {
    uart.write(b"RF2_INIT_BEGIN\r\n");
    uart.write(b"RF2_INIT_SKIPPED:full-init feature disabled\r\n");
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    efuse: hisi_hal::peripherals::Efuse<'static>,
) {
    let mut vendor_log = VendorLogGuard::new(uart);
    uart.write(b"RF2_INIT_BEGIN\r\n");
    let radio = match hisi_rf::init(
        ws63_rf_rs::hisi_rf_backend::config(),
        ws63_rf_rs::hisi_rf_backend::resources(efuse),
        &RADIO_STATE,
    ) {
        Ok(radio) => radio,
        Err(error) => {
            write_radio_error(uart, b"RF2_INIT_ERR", error);
            return;
        }
    };
    let hisi_rf::RadioParts { mut wifi, runner } = radio.split();
    let runner = RADIO_RUNNER.init(runner);
    if let Err(error) = hisi_rf_rtos_driver::spawn(
        radio_runner_task,
        runner as *mut Ws63RadioRunner as *mut core::ffi::c_void,
        hisi_rf_rtos_driver::TaskConfig {
            stack_size: core::num::NonZeroUsize::new(8 * 1024).unwrap(),
            priority: 10,
        },
    ) {
        write_radio_error(
            uart,
            b"RF2_INIT_ERR",
            hisi_rf::Error::Backend(hisi_rf::BackendError {
                class: hisi_rf::BackendErrorClass::Initialize,
                code: 0x3000_0000 | radio_runtime_error_code(error),
            }),
        );
        return;
    }
    if let Err(error) = radio_block_on(wifi.controller.initialize()) {
        write_radio_error(uart, b"RF2_INIT_ERR", error);
        dump_rtos_task_metrics();
        return;
    }
    uart.write(b"RF2_INIT_OK ifname=hisi-rf\r\n");
    write_radio_event(uart, radio_block_on(wifi.controller.next_event()));

    #[cfg(feature = "rf-queue-guard")]
    ws63_rf_rs::osal::arm_frw_queue_guard();

    uart.write(b"RF3_SCAN_BEGIN\r\n");
    let mut results = [hisi_rf::ScanResult::empty(); MAX_SCAN_RESULTS];
    let scan = match radio_block_on(wifi.controller.scan(
        hisi_rf::ScanConfig::try_from_timeout_ms(15_000).unwrap(),
        &mut results,
    )) {
        Ok(scan) => scan,
        Err(error) => {
            write_radio_error(uart, b"RF3_SCAN_ERR", error);
            return;
        }
    };
    uart.write(b"RF3_SCAN_OK count=0x");
    uart.write(&hex8(scan.count as u32));
    uart.write(b" truncated=0x");
    uart.write(&hex8(scan.truncated as u32));
    uart.write(b"\r\n");
    write_radio_event(uart, radio_block_on(wifi.controller.next_event()));
    for result in &results[..scan.count] {
        uart.write(b"RF3_AP ssid=");
        uart.write(result.ssid.as_bytes());
        uart.write(b" freq=0x");
        uart.write(&hex8(result.frequency_mhz as u32));
        uart.write(b" rssi=0x");
        uart.write(&hex8(result.rssi_dbm as i32 as u32));
        uart.write(b" bssid=");
        for byte in result.bssid {
            uart.write(&hex8(byte as u32)[6..]);
        }
        uart.write(b"\r\n");
    }
    #[cfg(feature = "upstream-supplicant")]
    {
        uart.write(b"W2D_NATIVE_RUNNER_RX_READY\r\n");
        let Some(result) = results[..scan.count]
            .iter()
            .find(|result| result.ssid.as_bytes() == TEST_SSID)
        else {
            uart.write(b"W2D_AP_NOT_FOUND ssid=");
            uart.write(TEST_SSID);
            uart.write(b"\r\n");
            return;
        };
        let Some(passphrase) = hisi_rf::Passphrase::try_from_ascii(TEST_PASSPHRASE) else {
            #[cfg(not(feature = "upstream-wpa3"))]
            uart.write(b"W2D_WPA_CONFIG_ERR:invalid-passphrase\r\n");
            #[cfg(feature = "upstream-wpa3")]
            uart.write(b"W2E_WPA3_CONFIG_ERR:invalid-passphrase\r\n");
            return;
        };
        #[cfg(feature = "upstream-wpa3")]
        match result.security {
            hisi_rf::Security::Wpa3Personal => {
                uart.write(b"W2E_AP_SECURITY mode=pure-wpa3\r\n");
            }
            hisi_rf::Security::Wpa2Wpa3PersonalTransition => {
                uart.write(b"W2E_AP_SECURITY mode=transition\r\n");
            }
            _ => {
                uart.write(b"W2E_WPA3_CONFIG_ERR:scan-security\r\n");
                return;
            }
        }
        #[cfg(not(feature = "upstream-wpa3"))]
        let network = hisi_rf::StationConfig::wpa2_personal(result, passphrase, 30_000);
        #[cfg(feature = "upstream-wpa3")]
        let network = hisi_rf::StationConfig::wpa3_personal(
            result,
            passphrase,
            hisi_rf::SaePwe::Both,
            30_000,
        );
        let Some(network) = network else {
            #[cfg(not(feature = "upstream-wpa3"))]
            uart.write(b"W2D_WPA_CONFIG_ERR:unsupported-security\r\n");
            #[cfg(feature = "upstream-wpa3")]
            uart.write(b"W2E_WPA3_CONFIG_ERR:unsupported-security\r\n");
            return;
        };
        uart.write(b"W2D_CONNECT_BEGIN ssid=");
        uart.write(network.ssid.as_bytes());
        uart.write(b"\r\n");
        match radio_block_on(wifi.controller.connect(network)) {
            Ok(info) => {
                #[cfg(not(feature = "upstream-wpa3"))]
                uart.write(b"W2D_WPA2_CONNECT_OK freq=0x");
                #[cfg(feature = "upstream-wpa3")]
                uart.write(b"W2E_WPA3_CONNECT_OK pmf=required freq=0x");
                uart.write(&hex8(info.frequency_mhz as u32));
                uart.write(b"\r\n");
                write_radio_event(uart, radio_block_on(wifi.controller.next_event()));
                vendor_log.flush();
                network_runner::run(uart, wifi.device);
            }
            Err(error) => {
                #[cfg(not(feature = "upstream-wpa3"))]
                write_radio_error(uart, b"W2D_WPA2_CONNECT_ERR", error);
                #[cfg(feature = "upstream-wpa3")]
                write_radio_error(uart, b"W2E_WPA3_CONNECT_ERR", error);
                #[cfg(feature = "upstream-supplicant")]
                write_upstream_supplicant_diagnostics(uart);
            }
        }
        dump_rtos_task_metrics();
    }
    #[cfg(feature = "personal")]
    {
        let Some(result) = results[..scan.count]
            .iter()
            .find(|result| result.ssid.as_bytes() == TEST_SSID)
        else {
            uart.write(b"RF5B_AP_NOT_FOUND ssid=");
            uart.write(TEST_SSID);
            uart.write(b"\r\n");
            return;
        };
        let Some(passphrase) = hisi_rf::Passphrase::try_from_ascii(TEST_PASSPHRASE) else {
            uart.write(b"RF5B_WPA_CONFIG_ERR:invalid-passphrase\r\n");
            return;
        };
        #[cfg(feature = "wpa")]
        let network = hisi_rf::StationConfig::wpa2_personal(result, passphrase, 30_000);
        #[cfg(feature = "wpa3")]
        let network = hisi_rf::StationConfig::wpa3_personal(
            result,
            passphrase,
            hisi_rf::SaePwe::Both,
            30_000,
        );
        let Some(network) = network else {
            uart.write(b"RF5B_WPA_CONFIG_ERR:unsupported-security\r\n");
            return;
        };
        uart.write(b"RF5B_CONNECT_BEGIN ssid=");
        uart.write(network.ssid.as_bytes());
        uart.write(b"\r\n");
        match radio_block_on(wifi.controller.connect(network)) {
            Ok(info) => {
                uart.write(b"RF5B_WPA_CONNECT_OK freq=0x");
                uart.write(&hex8(info.frequency_mhz as u32));
                uart.write(b"\r\n");
                #[cfg(feature = "wpa")]
                uart.write(b"W2_PROFILE_OK mode=wpa2-personal\r\n");
                #[cfg(feature = "wpa3")]
                uart.write(b"W2_PROFILE_OK mode=wpa3-personal\r\n");
                write_radio_event(uart, radio_block_on(wifi.controller.next_event()));
                vendor_log.flush();
                network_runner::run(uart, wifi.device);
            }
            Err(error) => {
                write_radio_error(uart, b"RF5B_WPA_CONNECT_ERR", error);
            }
        }
        dump_rtos_task_metrics();
    }
}

#[cfg(all(feature = "full-init", feature = "upstream-supplicant"))]
fn write_upstream_supplicant_diagnostics(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    let [
        flags_status,
        flags_lo,
        flags_hi,
        assoc_status,
        auth,
        pmf,
        pwe,
        akm,
    ] = ws63_rf_rs::upstream_supplicant_diagnostic_snapshot();
    uart.write(b"RFDBG_WPA_DRIVER_FLAGS status=0x");
    uart.write(&hex8(flags_status));
    uart.write(b" lo=0x");
    uart.write(&hex8(flags_lo));
    uart.write(b" hi=0x");
    uart.write(&hex8(flags_hi));
    uart.write(b"\r\nRFDBG_WPA_ASSOC_SUBMIT status=0x");
    uart.write(&hex8(assoc_status));
    uart.write(b" auth=0x");
    uart.write(&hex8(auth));
    uart.write(b" pmf=0x");
    uart.write(&hex8(pmf));
    uart.write(b" pwe=0x");
    uart.write(&hex8(pwe));
    uart.write(b" akm=0x");
    uart.write(&hex8(akm));
    uart.write(b"\r\n");
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
extern "C" fn radio_runner_task(argument: *mut core::ffi::c_void) -> *mut core::ffi::c_void {
    // SAFETY: `argument` comes from `RADIO_RUNNER.init`, remains live forever,
    // and this is the only task that receives its mutable pointer.
    let runner = unsafe { &mut *argument.cast::<Ws63RadioRunner>() };
    loop {
        let _ = runner.run_once();
        // `run_once` bounds one hostap batch; even a busy runner must yield so a
        // cooperative controller task can consume completions and so vendor
        // workers can deliver the next RX/timeout event.
        hisi_rf_rtos_driver::yield_now().expect("yield radio runner");
    }
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
fn radio_block_on<F: core::future::Future>(future: F) -> F::Output {
    use core::task::{Context, Poll, RawWaker, RawWakerVTable, Waker};

    static VTABLE: RawWakerVTable = RawWakerVTable::new(clone, wake, wake, drop_waker);
    const fn clone(_: *const ()) -> RawWaker {
        RawWaker::new(core::ptr::null(), &VTABLE)
    }
    const fn wake(_: *const ()) {}
    const fn drop_waker(_: *const ()) {}

    let mut future = core::pin::pin!(future);
    // SAFETY: the no-op vtable never dereferences its null data pointer. This
    // executor explicitly yields and polls again rather than relying on wake.
    let waker = unsafe { Waker::from_raw(clone(core::ptr::null())) };
    let mut context = Context::from_waker(&waker);
    loop {
        if let Poll::Ready(output) = future.as_mut().poll(&mut context) {
            return output;
        }
        hisi_rf_rtos_driver::yield_now().expect("yield radio controller");
    }
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
fn write_radio_event(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>, event: hisi_rf::WifiEvent) {
    uart.write(b"A4_RADIO_EVENT kind=");
    match event {
        hisi_rf::WifiEvent::Initialized => uart.write(b"initialized"),
        hisi_rf::WifiEvent::ScanCompleted { .. } => uart.write(b"scan-completed"),
        hisi_rf::WifiEvent::Connected(_) => uart.write(b"connected"),
        hisi_rf::WifiEvent::Disconnected { .. } => uart.write(b"disconnected"),
        hisi_rf::WifiEvent::Failed(_) => uart.write(b"failed"),
    }
    uart.write(b"\r\n");
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
fn write_radio_error(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    marker: &[u8],
    error: hisi_rf::Error,
) {
    uart.write(marker);
    let (class, code) = match error {
        hisi_rf::Error::AlreadyInitialized => (0_u32, 1_u32),
        hisi_rf::Error::Protocol => (0, 2),
        hisi_rf::Error::Backend(error) => (radio_error_class_code(error.class), error.code),
    };
    uart.write(b" class=0x");
    uart.write(&hex8(class));
    uart.write(b" code=0x");
    uart.write(&hex8(code));
    uart.write(b"\r\n");
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
const fn radio_error_class_code(class: hisi_rf::BackendErrorClass) -> u32 {
    match class {
        hisi_rf::BackendErrorClass::Initialize => 1,
        hisi_rf::BackendErrorClass::Busy => 2,
        hisi_rf::BackendErrorClass::Timeout => 3,
        hisi_rf::BackendErrorClass::UnsupportedSecurity => 4,
        hisi_rf::BackendErrorClass::Connect => 5,
        hisi_rf::BackendErrorClass::Other => 6,
    }
}

#[cfg(all(
    feature = "full-init",
    any(feature = "personal", feature = "upstream-supplicant")
))]
const fn radio_runtime_error_code(error: hisi_rf_rtos_driver::Error) -> u32 {
    match error {
        hisi_rf_rtos_driver::Error::NotInstalled => 1,
        hisi_rf_rtos_driver::Error::AlreadyInstalled => 2,
        hisi_rf_rtos_driver::Error::ResourceExhausted => 3,
        hisi_rf_rtos_driver::Error::NoTaskSlots => 4,
        hisi_rf_rtos_driver::Error::InvalidHandle => 5,
        hisi_rf_rtos_driver::Error::InvalidContext => 6,
        hisi_rf_rtos_driver::Error::TimedOut => 7,
        hisi_rf_rtos_driver::Error::Runtime => 8,
    }
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    efuse: hisi_hal::peripherals::Efuse<'_>,
) {
    uart.write(b"RF2_INIT_BEGIN\r\n");
    let mut wifi = match ActiveWifi::initialize(efuse) {
        Ok(wifi) => wifi,
        Err(error) => {
            write_wifi_error(uart, b"RF2_INIT_ERR", error);
            dump_rtos_task_metrics();
            return;
        }
    };
    uart.write(b"RF2_INIT_OK ifname=");
    uart.write(wifi.interface_name());
    if let Some(diag) = ws63_rf_rs::netif::diagnostics() {
        uart.write(b" netif=0x");
        uart.write(&hex8(diag.address as u32));
        uart.write(b" drv_send=0x");
        uart.write(&hex8(diag.driver_send as u32));
        uart.write(b" hwlen=0x");
        uart.write(&hex8(diag.hardware_address_len as u32));
    }
    if let Some(mac) = ws63_rf_rs::netif::hardware_address() {
        uart.write(b" mac=");
        for byte in mac {
            uart.write(&hex8(byte as u32)[6..]);
        }
    }
    uart.write(b"\r\n");

    #[cfg(feature = "rf-queue-guard")]
    ws63_rf_rs::osal::arm_frw_queue_guard();

    #[cfg(feature = "rf-watchpoint-gate")]
    {
        uart.write(b"RFDBG_SCAN_GATE addr=0x");
        uart.write(&hex8(core::ptr::addr_of!(__WS63_RF_SCAN_DEBUG_GATE) as u32));
        uart.write(b"\r\n");
        while unsafe { core::ptr::read_volatile(&raw const __WS63_RF_SCAN_DEBUG_GATE) } == 0 {
            core::hint::spin_loop();
        }
    }

    uart.write(b"RF3_SCAN_BEGIN\r\n");
    let mut event_baseline = [ws63_rf_rs::osal_queue::EventDiagnostic::default(); 16];
    let event_baseline_count = ws63_rf_rs::osal_queue::event_diagnostics(&mut event_baseline);
    for event in &event_baseline[..event_baseline_count] {
        uart.write(b"RFDBG_EVENT_BASE event=0x");
        uart.write(&hex8(event.event as u32));
        uart.write(b" writes=0x");
        uart.write(&hex8(event.writes));
        uart.write(b" matches=0x");
        uart.write(&hex8(event.matches));
        uart.write(b" bits=0x");
        uart.write(&hex8(event.bits));
        uart.write(b"\r\n");
    }
    let mut results = [ScanResult::empty(); MAX_SCAN_RESULTS];
    match wifi.scan(&mut results, 15_000) {
        Ok(count) => {
            uart.write(b"RF3_SCAN_OK count=0x");
            uart.write(&hex8(count as u32));
            uart.write(b"\r\n");
            for result in &results[..count] {
                uart.write(b"RF3_AP ssid=");
                uart.write(result.ssid());
                uart.write(b" freq=0x");
                uart.write(&hex8(result.frequency_mhz as u32));
                uart.write(b" rssi=0x");
                uart.write(&hex8(result.rssi_dbm as i32 as u32));
                uart.write(b" bssid=");
                for byte in result.bssid {
                    uart.write(&hex8(byte as u32)[6..]);
                }
                uart.write(b"\r\n");
            }
            let Some(result) = results[..count]
                .iter()
                .find(|result| result.ssid() == TEST_SSID)
            else {
                uart.write(b"RF5B_AP_NOT_FOUND ssid=");
                uart.write(TEST_SSID);
                uart.write(b"\r\n");
                return;
            };
            #[cfg(not(feature = "personal"))]
            let network = match OpenNetwork::from_scan(result) {
                Ok(network) => network,
                Err(error) => {
                    write_wifi_error(uart, b"RF5B_CONFIG_ERR", error);
                    return;
                }
            };
            #[cfg(feature = "personal")]
            let network = match PersonalNetwork::from_scan(result, TEST_PASSPHRASE) {
                Ok(network) => network,
                Err(error) => {
                    write_wifi_error(uart, b"RF5B_WPA_CONFIG_ERR", error);
                    return;
                }
            };
            uart.write(b"RF5B_CONNECT_BEGIN ssid=");
            uart.write(network.ssid());
            uart.write(b"\r\n");
            #[cfg(not(feature = "personal"))]
            match wifi.connect_open(&network, 15_000) {
                Ok(info) => {
                    uart.write(b"RF5B_CONNECT_OK freq=0x");
                    uart.write(&hex8(info.frequency_mhz as u32));
                    uart.write(b"\r\n");
                    run_arp_probe(uart);
                }
                Err(error) => write_wifi_error(uart, b"RF5B_CONNECT_ERR", error),
            }
            #[cfg(feature = "personal")]
            match wifi.connect(&network, 30_000) {
                Ok(info) => {
                    uart.write(b"RF5B_WPA_CONNECT_OK freq=0x");
                    uart.write(&hex8(info.frequency_mhz as u32));
                    uart.write(b"\r\n");
                    run_arp_probe(uart);
                }
                Err(error) => write_wifi_error(uart, b"RF5B_WPA_CONNECT_ERR", error),
            }
        }
        Err(error) => write_wifi_error(uart, b"RF3_SCAN_ERR", error),
    }
    for irq in [40, 44, 45] {
        uart.write(b"RFDBG_IRQ_COUNT irq=0x");
        uart.write(&hex8(irq));
        uart.write(b" count=0x");
        uart.write(&hex8(ws63_rf_rs::osal::irq_dispatch_count(irq)));
        uart.write(b"\r\n");
    }
    let diag = hisi_rtos::diagnostics();
    uart.write(b"RFDBG_RTOS switches=0x");
    uart.write(&hex8(diag.context_switches));
    uart.write(b" irq_preemptions=0x");
    uart.write(&hex8(diag.irq_preemptions));
    uart.write(b" timer_irqs=0x");
    uart.write(&hex8(diag.timer_interrupts));
    uart.write(b" slice_preemptions=0x");
    uart.write(&hex8(diag.time_slice_preemptions));
    uart.write(b" software_irqs=0x");
    uart.write(&hex8(diag.software_interrupts));
    uart.write(b" switch_race_recoveries=0x");
    uart.write(&hex8(diag.switch_race_recoveries));
    uart.write(b" yields=0x");
    uart.write(&hex8(diag.yields));
    uart.write(b" sleeps=0x");
    uart.write(&hex8(diag.sleeps));
    uart.write(b" sem_blocks=0x");
    uart.write(&hex8(diag.semaphore_blocks));
    uart.write(b" sem_wakes=0x");
    uart.write(&hex8(diag.semaphore_wakes));
    uart.write(b" sem_timeouts=0x");
    uart.write(&hex8(diag.semaphore_timeouts));
    uart.write(b" ready=0x");
    uart.write(&hex8(diag.ready_tasks as u32));
    uart.write(b" blocked=0x");
    uart.write(&hex8(diag.blocked_tasks as u32));
    uart.write(b" sleeping=0x");
    uart.write(&hex8(diag.sleeping_tasks as u32));
    uart.write(b"\r\n");
    let mut waits = [ws63_rf_rs::osal_wait::WaitDiagnostic::default(); 16];
    let wait_count = ws63_rf_rs::osal_wait::wait_diagnostics(&mut waits);
    for wait in &waits[..wait_count] {
        uart.write(b"RFDBG_WAIT wait=0x");
        uart.write(&hex8(wait.wait as u32));
        uart.write(b" sem=0x");
        uart.write(&hex8(wait.semaphore as u32));
        uart.write(b" pred=0x");
        uart.write(&hex8(wait.predicate as u32));
        uart.write(b" param=0x");
        uart.write(&hex8(wait.parameter as u32));
        uart.write(b" pred_now=0x");
        uart.write(&hex8(wait.predicate_result as u32));
        uart.write(b" blocks=0x");
        uart.write(&hex8(wait.blocks));
        uart.write(b" wakeups=0x");
        uart.write(&hex8(wait.wakeups));
        uart.write(b" ready=0x");
        uart.write(&hex8(wait.ready_checks));
        uart.write(b" wait_task=0x");
        uart.write(&hex8(wait.last_wait_task as u32));
        uart.write(b" wake_task=0x");
        uart.write(&hex8(wait.last_wake_task as u32));
        uart.write(b" wait_ra=0x");
        uart.write(&hex8(wait.last_wait_caller as u32));
        uart.write(b" wake_ra=0x");
        uart.write(&hex8(wait.last_wake_caller as u32));
        uart.write(b"\r\n");
    }
    let mut events = [ws63_rf_rs::osal_queue::EventDiagnostic::default(); 16];
    let event_count = ws63_rf_rs::osal_queue::event_diagnostics(&mut events);
    for event in &events[..event_count] {
        uart.write(b"RFDBG_EVENT event=0x");
        uart.write(&hex8(event.event as u32));
        uart.write(b" bits=0x");
        uart.write(&hex8(event.bits));
        uart.write(b" reads=0x");
        uart.write(&hex8(event.reads));
        uart.write(b" writes=0x");
        uart.write(&hex8(event.writes));
        uart.write(b" matches=0x");
        uart.write(&hex8(event.matches));
        uart.write(b" read_mask=0x");
        uart.write(&hex8(event.last_read_mask));
        uart.write(b" write_mask=0x");
        uart.write(&hex8(event.last_write_mask));
        uart.write(b" mode=0x");
        uart.write(&hex8(event.last_mode));
        uart.write(b"\r\n");
    }
    #[cfg(feature = "rf-eloop-diag")]
    {
        let mut eloops = [ws63_rf_rs::eloop_diag::EloopDiagnostic::default(); 16];
        let eloop_count = ws63_rf_rs::eloop_diag::diagnostics(&mut eloops);
        for eloop in &eloops[..eloop_count] {
            uart.write(b"RFDBG_ELOOP event=0x");
            uart.write(&hex8(eloop.event as u32));
            uart.write(b" posts=0x");
            uart.write(&hex8(eloop.posts));
            uart.write(b" post_fail=0x");
            uart.write(&hex8(eloop.post_failures));
            uart.write(b" reads=0x");
            uart.write(&hex8(eloop.reads));
            uart.write(b" nonempty=0x");
            uart.write(&hex8(eloop.nonempty_reads));
            uart.write(b" post_ra=0x");
            uart.write(&hex8(eloop.last_post_caller as u32));
            uart.write(b" read_ra=0x");
            uart.write(&hex8(eloop.last_read_caller as u32));
            uart.write(b" buffer=0x");
            uart.write(&hex8(eloop.last_buffer as u32));
            uart.write(b"\r\n");
        }
        let mut driver_events = [ws63_rf_rs::eloop_diag::DriverEventDiagnostic::default(); 32];
        let driver_event_count = ws63_rf_rs::eloop_diag::driver_events(&mut driver_events);
        for event in &driver_events[..driver_event_count] {
            uart.write(b"RFDBG_DRIVER_EVENT seq=0x");
            uart.write(&hex8(event.sequence));
            uart.write(b" cmd=0x");
            uart.write(&hex8(event.command));
            uart.write(b" len=0x");
            uart.write(&hex8(event.length));
            uart.write(b" payload0=0x");
            uart.write(&hex8(event.payload0));
            uart.write(b"\r\n");
        }
        let event = ws63_rf_rs::eloop_diag::supplicant_event();
        uart.write(b"RFDBG_SUPPLICANT_EVENT calls=0x");
        uart.write(&hex8(event.calls));
        uart.write(b" event=0x");
        uart.write(&hex8(event.event as u32));
        uart.write(b" ctx=0x");
        uart.write(&hex8(event.context as u32));
        uart.write(b" wifi_dev=0x");
        uart.write(&hex8(event.wifi_device as u32));
        uart.write(b" eloop=0x");
        uart.write(&hex8(event.eloop_status as u32));
        uart.write(b"\r\n");
        let dispatch = ws63_rf_rs::eloop_diag::driver_dispatch();
        uart.write(b"RFDBG_DRIVER_DISPATCH samples=0x");
        uart.write(&hex8(dispatch.samples));
        uart.write(b" caller=0x");
        uart.write(&hex8(dispatch.caller as u32));
        uart.write(b" cmd_reg=0x");
        uart.write(&hex8(dispatch.command_register as u32));
        uart.write(b" len_reg=0x");
        uart.write(&hex8(dispatch.length_register as u32));
        uart.write(b"\r\n");
        let auth = ws63_rf_rs::eloop_diag::auth();
        uart.write(b"RFDBG_AUTH dmac_rx=0x");
        uart.write(&hex8(auth.dmac_rx_calls));
        uart.write(b" dmac_auth=0x");
        uart.write(&hex8(auth.dmac_rx_auth_frames));
        uart.write(b" dmac_seq2=0x");
        uart.write(&hex8(auth.dmac_rx_auth_seq2_frames));
        uart.write(b" ingress=0x");
        uart.write(&hex8(auth.hmac_ingress_calls));
        uart.write(b" ingress_auth=0x");
        uart.write(&hex8(auth.hmac_ingress_auth_frames));
        uart.write(b" ingress_seq2=0x");
        uart.write(&hex8(auth.hmac_ingress_auth_seq2_frames));
        uart.write(b" tx_auth=0x");
        uart.write(&hex8(auth.tx_auth_frames));
        uart.write(b" tx_alg=0x");
        uart.write(&hex8(auth.tx_algorithm as u32));
        uart.write(b" tx_seq=0x");
        uart.write(&hex8(auth.tx_sequence as u32));
        uart.write(b" tx_netbuf=0x");
        uart.write(&hex8(auth.tx_netbuf as u32));
        uart.write(b" tx_comp=0x");
        uart.write(&hex8(auth.tx_complete_calls));
        uart.write(b" tx_comp_after_auth=0x");
        uart.write(&hex8(auth.tx_complete_after_auth));
        uart.write(b" tx_comp_skb=0x");
        uart.write(&hex8(auth.tx_complete_skb as u32));
        uart.write(b" tx_comp_frame=0x");
        uart.write(&hex8(auth.tx_complete_frame as u32));
        uart.write(b" tx_status=0x");
        uart.write(&hex8(auth.tx_complete_status as u32));
        uart.write(b" tx_counts=0x");
        uart.write(&hex8(auth.tx_complete_data_counts as u32));
        uart.write(b" auth_tx_comp=0x");
        uart.write(&hex8(auth.auth_tx_complete_calls));
        uart.write(b" auth_tx_status=0x");
        uart.write(&hex8(auth.auth_tx_status as u32));
        uart.write(b" auth_tx_counts=0x");
        uart.write(&hex8(auth.auth_tx_data_counts as u32));
        uart.write(b" wait_calls=0x");
        uart.write(&hex8(auth.wait_state_calls));
        uart.write(b" auth_frames=0x");
        uart.write(&hex8(auth.auth_frames));
        uart.write(b" auth_seq2=0x");
        uart.write(&hex8(auth.auth_seq2_frames));
        uart.write(b" alg=0x");
        uart.write(&hex8(auth.last_algorithm as u32));
        uart.write(b" seq=0x");
        uart.write(&hex8(auth.last_sequence as u32));
        uart.write(b" status=0x");
        uart.write(&hex8(auth.last_status as u32));
        uart.write(b" result=0x");
        uart.write(&hex8(auth.last_handler_result));
        uart.write(b" timeouts=0x");
        uart.write(&hex8(auth.timeout_calls));
        uart.write(b" first_seq2_st=0x");
        uart.write(&hex8(auth.first_auth_seq2_systick_ms as u32));
        uart.write(b" last_seq2_st=0x");
        uart.write(&hex8(auth.last_auth_seq2_systick_ms as u32));
        uart.write(b" first_seq2_tcxo=0x");
        uart.write(&hex8(auth.first_auth_seq2_tcxo_ms as u32));
        uart.write(b" last_seq2_tcxo=0x");
        uart.write(&hex8(auth.last_auth_seq2_tcxo_ms as u32));
        uart.write(b"\r\n");
        for (name, address) in [
            (b"RFDBG_AUTH_TX_DA=".as_slice(), auth.tx_destination),
            (b"RFDBG_AUTH_TX_SA=".as_slice(), auth.tx_source),
            (b"RFDBG_AUTH_TX_BSSID=".as_slice(), auth.tx_bssid),
        ] {
            uart.write(name);
            for byte in address {
                uart.write(&hex8(byte as u32)[6..]);
            }
            uart.write(b"\r\n");
        }
        uart.write(b"RFDBG_AUTH_TX_COMPLETE_WORDS=");
        for word in auth.last_tx_complete_words {
            uart.write(&hex8(word));
            uart.write(b",");
        }
        uart.write(b"\r\n");
        uart.write(b"RFDBG_BRIDGE_XMIT calls=0x");
        uart.write(&hex8(auth.bridge_xmit_calls));
        uart.write(b" result=0x");
        uart.write(&hex8(auth.bridge_xmit_result as u32));
        uart.write(b" skb=0x");
        uart.write(&hex8(auth.bridge_xmit_skb as u32));
        uart.write(b"\r\n");
        uart.write(b"RFDBG_NETIF_RX calls=0x");
        uart.write(&hex8(auth.netif_rx_calls));
        uart.write(b" len=0x");
        uart.write(&hex8(auth.netif_rx_length));
        uart.write(b" bytes=");
        for byte in auth.netif_rx_prefix {
            uart.write(&hex8(byte as u32)[6..]);
        }
        uart.write(b"\r\n");
        uart.write(b"RFDBG_TX_COMPLETE_FRAME=");
        for byte in auth.tx_complete_frame_prefix {
            uart.write(&hex8(byte as u32)[6..]);
        }
        uart.write(b"\r\n");
        uart.write(b"RFDBG_AUTH_TX_FRAME len=0x");
        uart.write(&hex8(auth.tx_frame_len as u32));
        uart.write(b" bytes=");
        for byte in &auth.tx_frame[..usize::min(auth.tx_frame_len as usize, auth.tx_frame.len())] {
            uart.write(&hex8(*byte as u32)[6..]);
        }
        uart.write(b"\r\n");
        uart.write(b"RFDBG_AUTH_TX_MAC_FILTER vap=0x");
        uart.write(&hex8(auth.tx_vap_id as u32));
        uart.write(b" combined=0x");
        uart.write(&hex8(auth.tx_mac_filter.combined));
        uart.write(b" station_tail=0x");
        uart.write(&hex8(auth.tx_mac_filter.station_tail));
        uart.write(b" bssid_tail=0x");
        uart.write(&hex8(auth.tx_mac_filter.bssid_tail));
        uart.write(b" rx_filter_before=0x");
        uart.write(&hex8(auth.rx_filter_before_tx));
        uart.write(b" rx_filter_after=0x");
        uart.write(&hex8(auth.rx_filter_after_override));
        uart.write(b"\r\n");
        for (name, statistics) in [
            (
                b"RFDBG_AUTH_RX_STATS_TX".as_slice(),
                auth.rx_statistics_at_tx,
            ),
            (
                b"RFDBG_AUTH_RX_STATS_TIMEOUT".as_slice(),
                auth.rx_statistics_at_timeout,
            ),
        ] {
            uart.write(name);
            uart.write(b" success=0x");
            uart.write(&hex8(statistics.successful_mpdu));
            uart.write(b" error=0x");
            uart.write(&hex8(statistics.failed_mpdu));
            uart.write(b" filtered=0x");
            uart.write(&hex8(statistics.filtered_mpdu));
            uart.write(b" ampdu=0x");
            uart.write(&hex8(statistics.ampdu));
            uart.write(b"\r\n");
        }
        if let Some(filter) = ws63_rf_rs::eloop_diag::mac_filter(1) {
            uart.write(b"RFDBG_MAC_FILTER vap=1 combined=0x");
            uart.write(&hex8(filter.combined));
            uart.write(b" station_tail=0x");
            uart.write(&hex8(filter.station_tail));
            uart.write(b" bssid_tail=0x");
            uart.write(&hex8(filter.bssid_tail));
            uart.write(b"\r\n");
        }
        for index in 0..usize::min(auth.timeout_calls as usize, auth.timeout_systick_ms.len()) {
            uart.write(b"RFDBG_AUTH_TIMEOUT index=0x");
            uart.write(&hex8(index as u32));
            uart.write(b" systick_ms=0x");
            uart.write(&hex8(auth.timeout_systick_ms[index] as u32));
            uart.write(b" tcxo_ms=0x");
            uart.write(&hex8(auth.timeout_tcxo_ms[index] as u32));
            uart.write(b"\r\n");
        }
        #[cfg(feature = "personal")]
        {
            let event = ws63_rf_rs::wifi::wpa_event_diagnostics();
            uart.write(b"RFDBG_WPA_EVENT calls=0x");
            uart.write(&hex8(event.calls));
            uart.write(b" last_kind=0x");
            uart.write(&hex8(event.last_kind as u32));
            uart.write(b" scan_events=0x");
            uart.write(&hex8(event.scan_events));
            uart.write(b" active=0x");
            uart.write(&hex8(event.scan_active_on_event as u32));
            uart.write(b" published=0x");
            uart.write(&hex8(event.scan_done_published as u32));
            uart.write(b" vendor_flag=0x");
            uart.write(&hex8(event.vendor_scan_flag as u32));
            uart.write(b" callback=0x");
            uart.write(&hex8(event.registered_callback as u32));
            uart.write(b"\r\n");
        }
    }
    dump_rtos_task_metrics();
}

#[cfg(feature = "full-init")]
fn dump_rtos_task_metrics() {
    let diag = hisi_rtos::diagnostics();
    rf_log_uart0(b"RFDBG_RTOS_SWITCH_RACE_RECOVERIES=0x");
    rf_log_uart0(&hex8(diag.switch_race_recoveries));
    rf_log_uart0(b"\r\n");
    let mut tasks = [hisi_rtos::TaskDiagnostic::default(); 32];
    let task_count = hisi_rtos::task_diagnostics(&mut tasks);
    for task in &tasks[..task_count] {
        if task.state == hisi_rtos::TaskState::Free {
            continue;
        }
        rf_log_uart0(b"RFDBG_TASK id=0x");
        rf_log_uart0(&hex8(task.task as u32));
        rf_log_uart0(b" state=0x");
        rf_log_uart0(&hex8(task.state as u32));
        rf_log_uart0(b" entry=0x");
        rf_log_uart0(&hex8(task.entry as u32));
        rf_log_uart0(b" base_priority=0x");
        rf_log_uart0(&hex8(task.base_priority as u32));
        rf_log_uart0(b" priority=0x");
        rf_log_uart0(&hex8(task.priority as u32));
        rf_log_uart0(b" sem=0x");
        rf_log_uart0(&hex8(task.waiting_sem as u32));
        rf_log_uart0(b" mutex=0x");
        rf_log_uart0(&hex8(task.waiting_mutex as u32));
        rf_log_uart0(b" wake_at=0x");
        rf_log_uart0(&hex16(task.wake_at));
        rf_log_uart0(b" lock=0x");
        rf_log_uart0(&hex8(task.scheduler_lock_depth as u32));
        rf_log_uart0(b"\r\n");

        let policy = match task.run_policy {
            hisi_rtos::RunPolicy::Cooperative => 0,
            hisi_rtos::RunPolicy::Budgeted(_) => 1,
            hisi_rtos::RunPolicy::Preemptive { .. } => 2,
        };
        rf_log_uart0(b"RFDBG_TASK_METRIC id=0x");
        rf_log_uart0(&hex8(task.task as u32));
        rf_log_uart0(b" policy=0x");
        rf_log_uart0(&hex8(policy));
        rf_log_uart0(b" cpu_ms=0x");
        rf_log_uart0(&hex16(task.cpu_time_ms));
        rf_log_uart0(b" irq_ms=0x");
        rf_log_uart0(&hex16(task.irq_time_ms));
        rf_log_uart0(b" dispatches=0x");
        rf_log_uart0(&hex8(task.dispatches));
        rf_log_uart0(b" budget_exhaustions=0x");
        rf_log_uart0(&hex8(task.budget_exhaustions));
        rf_log_uart0(b" budget_remaining=0x");
        rf_log_uart0(&hex8(task.budget_remaining));
        rf_log_uart0(b" max_run_ms=0x");
        rf_log_uart0(&hex16(task.max_continuous_run_ms));
        rf_log_uart0(b" max_ready_ms=0x");
        rf_log_uart0(&hex16(task.max_ready_latency_ms));
        rf_log_uart0(b" lock_entries=0x");
        rf_log_uart0(&hex8(task.scheduler_lock_entries));
        rf_log_uart0(b" max_lock_ms=0x");
        rf_log_uart0(&hex16(task.max_scheduler_lock_ms));
        rf_log_uart0(b" irq_entries=0x");
        rf_log_uart0(&hex8(task.irq_entries));
        rf_log_uart0(b" max_irq_ms=0x");
        rf_log_uart0(&hex16(task.max_irq_span_ms));
        rf_log_uart0(b"\r\n");
    }
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn run_arp_probe(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    let Some(mac) = ws63_rf_rs::netif::hardware_address() else {
        uart.write(b"RF5A_ARP_ERR:no-mac\r\n");
        return;
    };
    uart.write(b"RF5A_DHCP_BEGIN\r\n");
    #[cfg(feature = "rf-queue-guard")]
    ws63_rf_rs::netif::arm_host_queue_callback_watchpoint();
    let dhcp_started_at = ws63_rf_rs::uapi::monotonic_ms();
    let Some(config) = ws63_rf_rs::netif_smoltcp::dhcp_probe(mac, 30_000) else {
        uart.write(b"RFDBG_DHCP_ELAPSED_MS=0x");
        uart.write(&hex8(
            ws63_rf_rs::uapi::monotonic_ms().wrapping_sub(dhcp_started_at) as u32,
        ));
        uart.write(b"\r\n");
        uart.write(b"RF5A_DHCP_TIMEOUT rx=0x");
        uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
        uart.write(b" tx=0x");
        uart.write(&hex8(ws63_rf_rs::netif_smoltcp::tx_count()));
        uart.write(b" tx_failed=0x");
        uart.write(&hex8(ws63_rf_rs::netif::tx_failed()));
        uart.write(b" rx_dropped=0x");
        uart.write(&hex8(ws63_rf_rs::netif::rx_dropped()));
        uart.write(b"\r\n");
        let mut prefix = [0_u8; 64];
        let rx_len = ws63_rf_rs::netif_smoltcp::last_rx(&mut prefix);
        write_frame_prefix(uart, b"RFDBG_DHCP_RX", rx_len, &prefix);
        let tx_len = ws63_rf_rs::netif_smoltcp::last_tx(&mut prefix);
        write_frame_prefix(
            uart,
            b"RFDBG_DHCP_TX",
            tx_len,
            &prefix[..tx_len.min(prefix.len())],
        );
        run_static_arp_diagnostic(uart, mac);
        return;
    };
    uart.write(b"RFDBG_DHCP_ELAPSED_MS=0x");
    uart.write(&hex8(
        ws63_rf_rs::uapi::monotonic_ms().wrapping_sub(dhcp_started_at) as u32,
    ));
    uart.write(b"\r\n");
    uart.write(b"RF5A_DHCP_OK addr=");
    write_ipv4(uart, config.address);
    uart.write(b" prefix=0x");
    uart.write(&hex8(config.prefix_len as u32));
    let Some(gateway) = config.router else {
        uart.write(b" router=none\r\n");
        return;
    };
    uart.write(b" router=");
    write_ipv4(uart, gateway);
    uart.write(b"\r\n");

    let mut request = [0_u8; 74];
    request[..6].fill(0xff);
    request[6..12].copy_from_slice(&mac);
    request[12..14].copy_from_slice(&[0x08, 0x06]);
    request[14..22].copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 6, 4, 0x00, 0x01]);
    request[22..28].copy_from_slice(&mac);
    request[28..32].copy_from_slice(&config.address);
    request[38..42].copy_from_slice(&gateway);

    uart.write(b"RF5A_ARP_BEGIN target=");
    write_ipv4(uart, gateway);
    uart.write(b"\r\n");
    if ws63_rf_rs::netif::transmit(&request).is_err() {
        uart.write(b"RF5A_ARP_ERR:tx\r\n");
        return;
    }

    let mut frame = [0_u8; ws63_rf_rs::netif_smoltcp::MTU];
    for _ in 0..300 {
        if let Some(length) = ws63_rf_rs::netif_smoltcp::take_received(&mut frame)
            && length >= 42
            && frame[12..14] == [0x08, 0x06]
            && frame[20..22] == [0x00, 0x02]
            && frame[28..32] == gateway
        {
            uart.write(b"RF5A_ARP_OK rx=0x");
            uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
            uart.write(b"\r\n");
            let mut gateway_mac = [0_u8; 6];
            gateway_mac.copy_from_slice(&frame[6..12]);
            let gateway_ping =
                run_ping_series(uart, mac, config.address, gateway, gateway_mac, gateway);
            let public_ping = run_ping_series(
                uart,
                mac,
                config.address,
                gateway,
                gateway_mac,
                [1, 1, 1, 1],
            );
            uart.write(b"RF5C_CONNECTIVITY_SUMMARY gateway_tx=0x");
            uart.write(&hex8(gateway_ping.tx));
            uart.write(b" gateway_rx=0x");
            uart.write(&hex8(gateway_ping.rx));
            uart.write(b" public_tx=0x");
            uart.write(&hex8(public_ping.tx));
            uart.write(b" public_rx=0x");
            uart.write(&hex8(public_ping.rx));
            uart.write(b"\r\n");
            return;
        }
        ws63_rf_rs::osal::osal_msleep(10);
    }
    uart.write(b"RF5A_ARP_TIMEOUT rx=0x");
    uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
    uart.write(b" tx_failed=0x");
    uart.write(&hex8(ws63_rf_rs::netif::tx_failed()));
    uart.write(b"\r\n");
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn run_static_arp_diagnostic(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>, mac: [u8; 6]) {
    const ADDRESS: [u8; 4] = [192, 168, 155, 2];
    const GATEWAY: [u8; 4] = [192, 168, 155, 1];
    let mut request = [0_u8; 42];
    request[..6].fill(0xff);
    request[6..12].copy_from_slice(&mac);
    request[12..14].copy_from_slice(&[0x08, 0x06]);
    request[14..22].copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 0x06, 0x04, 0x00, 0x01]);
    request[22..28].copy_from_slice(&mac);
    request[28..32].copy_from_slice(&ADDRESS);
    request[38..42].copy_from_slice(&GATEWAY);
    uart.write(b"RFDBG_STATIC_ARP_BEGIN target=192.168.155.1\r\n");
    if ws63_rf_rs::netif::transmit(&request).is_err() {
        uart.write(b"RFDBG_STATIC_ARP_ERR:tx\r\n");
        return;
    }
    let mut frame = [0_u8; ws63_rf_rs::netif_smoltcp::MTU];
    for _ in 0..200 {
        if let Some(length) = ws63_rf_rs::netif_smoltcp::take_received(&mut frame) {
            write_frame_prefix(uart, b"RFDBG_STATIC_ARP_RX", length, &frame[..length]);
            if length >= 42
                && frame[12..14] == [0x08, 0x06]
                && frame[20..22] == [0x00, 0x02]
                && frame[28..32] == GATEWAY
                && frame[38..42] == ADDRESS
            {
                uart.write(b"RFDBG_STATIC_ARP_OK\r\n");
                return;
            }
        }
        ws63_rf_rs::osal::osal_msleep(10);
    }
    uart.write(b"RFDBG_STATIC_ARP_TIMEOUT\r\n");
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn write_frame_prefix(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    marker: &[u8],
    full_len: usize,
    prefix: &[u8],
) {
    uart.write(marker);
    uart.write(b" len=0x");
    uart.write(&hex8(full_len as u32));
    uart.write(b" bytes=");
    for byte in prefix {
        uart.write(&hex8(*byte as u32)[6..]);
    }
    uart.write(b"\r\n");
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
#[derive(Clone, Copy, Default)]
struct PingStats {
    tx: u32,
    rx: u32,
    tx_errors: u32,
    rx_queue_dropped: u32,
    rx_queue_high_watermark: u32,
    rx_queue_pending: u32,
    rx_echo_replies: u32,
    rx_echo_sequence_mask: u32,
    rtt_total_ms: u64,
    rtt_min_ms: u32,
    rtt_max_ms: u32,
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn run_ping_series(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    mac: [u8; 6],
    address: [u8; 4],
    gateway: [u8; 4],
    gateway_mac: [u8; 6],
    target: [u8; 4],
) -> PingStats {
    const IDENTIFIER: u16 = 0x5753;
    const SAMPLE_COUNT: u16 = 5;
    const SAMPLE_TIMEOUT_STEPS: usize = 100;
    // Ethernet (14) + IPv4 (20) + ICMP echo header (8) + payload (32).
    let mut request = [0_u8; 74];
    request[..6].copy_from_slice(&gateway_mac);
    request[6..12].copy_from_slice(&mac);
    request[12..14].copy_from_slice(&[0x08, 0x00]);
    request[14] = 0x45;
    let ip_packet_len = (request.len() - 14) as u16;
    request[16..18].copy_from_slice(&ip_packet_len.to_be_bytes());
    request[22] = 64;
    request[23] = 1;
    request[26..30].copy_from_slice(&address);
    request[30..34].copy_from_slice(&target);
    request[34] = 8;
    request[38..40].copy_from_slice(&IDENTIFIER.to_be_bytes());
    for (index, byte) in request[42..].iter_mut().enumerate() {
        *byte = index as u8;
    }

    uart.write(b"RF5C_PING_SERIES_BEGIN target=");
    write_ipv4(uart, target);
    uart.write(b" via=");
    write_ipv4(uart, gateway);
    uart.write(b" count=0x");
    uart.write(&hex8(SAMPLE_COUNT as u32));
    uart.write(b"\r\n");

    ws63_rf_rs::netif_smoltcp::reset_rx_queue_diagnostics();
    let mut stats = PingStats::default();
    let mut frame = [0_u8; ws63_rf_rs::netif_smoltcp::MTU];
    for sequence in 1..=SAMPLE_COUNT {
        request[18..20].copy_from_slice(&sequence.to_be_bytes());
        request[24..26].fill(0);
        let ip_checksum = internet_checksum(&request[14..34]);
        request[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
        request[40..42].copy_from_slice(&sequence.to_be_bytes());
        request[36..38].fill(0);
        let icmp_checksum = internet_checksum(&request[34..]);
        request[36..38].copy_from_slice(&icmp_checksum.to_be_bytes());

        uart.write(b"RF5C_PING_SAMPLE target=");
        write_ipv4(uart, target);
        uart.write(b" seq=0x");
        uart.write(&hex8(sequence as u32));
        let started_at = ws63_rf_rs::uapi::monotonic_ms();
        if ws63_rf_rs::netif::transmit(&request).is_err() {
            stats.tx_errors = stats.tx_errors.saturating_add(1);
            uart.write(b" status=tx_error\r\n");
            continue;
        }
        stats.tx = stats.tx.saturating_add(1);

        let mut received = false;
        for _ in 0..SAMPLE_TIMEOUT_STEPS {
            if let Some(length) = ws63_rf_rs::netif_smoltcp::take_received(&mut frame) {
                if length >= 42
                    && frame[12..14] == [0x08, 0x06]
                    && frame[20..22] == [0x00, 0x01]
                    && frame[38..42] == address
                {
                    let mut reply = [0_u8; 42];
                    reply[..6].copy_from_slice(&frame[22..28]);
                    reply[6..12].copy_from_slice(&mac);
                    reply[12..14].copy_from_slice(&[0x08, 0x06]);
                    reply[14..22]
                        .copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 0x06, 0x04, 0x00, 0x02]);
                    reply[22..28].copy_from_slice(&mac);
                    reply[28..32].copy_from_slice(&address);
                    reply[32..38].copy_from_slice(&frame[22..28]);
                    reply[38..42].copy_from_slice(&frame[28..32]);
                    let _ = ws63_rf_rs::netif::transmit(&reply);
                    continue;
                }
                if length >= 42
                    && frame[12..14] == [0x08, 0x00]
                    && frame[23] == 1
                    && frame[26..30] == target
                    && frame[30..34] == address
                    && frame[34] == 0
                    && frame[38..40] == IDENTIFIER.to_be_bytes()
                    && frame[40..42] == sequence.to_be_bytes()
                {
                    let rtt_ms = ws63_rf_rs::uapi::monotonic_ms()
                        .wrapping_sub(started_at)
                        .min(u32::MAX as u64) as u32;
                    stats.rx = stats.rx.saturating_add(1);
                    stats.rtt_total_ms = stats.rtt_total_ms.saturating_add(rtt_ms as u64);
                    stats.rtt_min_ms = if stats.rx == 1 {
                        rtt_ms
                    } else {
                        stats.rtt_min_ms.min(rtt_ms)
                    };
                    stats.rtt_max_ms = stats.rtt_max_ms.max(rtt_ms);
                    uart.write(b" status=ok rtt_ms=0x");
                    uart.write(&hex8(rtt_ms));
                    uart.write(b"\r\n");
                    received = true;
                    break;
                }
            }
            ws63_rf_rs::osal::osal_msleep(10);
        }
        if !received {
            uart.write(b" status=timeout\r\n");
        }
    }

    let drops = stats.tx.saturating_sub(stats.rx);
    let rx_queue = ws63_rf_rs::netif_smoltcp::rx_queue_diagnostics();
    stats.rx_queue_dropped = rx_queue.dropped;
    stats.rx_queue_high_watermark = rx_queue.high_watermark as u32;
    stats.rx_queue_pending = rx_queue.pending as u32;
    stats.rx_echo_replies = rx_queue.icmp_echo_replies;
    stats.rx_echo_sequence_mask = rx_queue.icmp_sequence_mask;
    let loss_pct = drops
        .saturating_mul(100)
        .checked_div(stats.tx)
        .unwrap_or(100);
    if stats.rx == 0 {
        uart.write(b"RF5C_PING_TIMEOUT target=");
    } else {
        uart.write(b"RF5C_PING_OK target=");
    }
    write_ipv4(uart, target);
    uart.write(b" tx=0x");
    uart.write(&hex8(stats.tx));
    uart.write(b" rx=0x");
    uart.write(&hex8(stats.rx));
    uart.write(b" drop=0x");
    uart.write(&hex8(drops));
    uart.write(b" tx_error=0x");
    uart.write(&hex8(stats.tx_errors));
    uart.write(b" rx_queue_drop=0x");
    uart.write(&hex8(stats.rx_queue_dropped));
    uart.write(b" rx_queue_high_water=0x");
    uart.write(&hex8(stats.rx_queue_high_watermark));
    uart.write(b" rx_queue_pending=0x");
    uart.write(&hex8(stats.rx_queue_pending));
    uart.write(b" rx_echo_replies=0x");
    uart.write(&hex8(stats.rx_echo_replies));
    uart.write(b" rx_echo_sequence_mask=0x");
    uart.write(&hex8(stats.rx_echo_sequence_mask));
    uart.write(b" loss_pct=0x");
    uart.write(&hex8(loss_pct));
    uart.write(b" rtt_min_ms=0x");
    uart.write(&hex8(stats.rtt_min_ms));
    uart.write(b" rtt_avg_ms=0x");
    uart.write(&hex8(
        stats
            .rtt_total_ms
            .checked_div(stats.rx as u64)
            .unwrap_or(0)
            .min(u32::MAX as u64) as u32,
    ));
    uart.write(b" rtt_max_ms=0x");
    uart.write(&hex8(stats.rtt_max_ms));
    uart.write(b"\r\n");
    stats
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    let (chunks, remainder) = bytes.as_chunks::<2>();
    for chunk in chunks {
        sum += u16::from_be_bytes(*chunk) as u32;
    }
    if let Some(&last) = remainder.first() {
        sum += (last as u32) << 8;
    }
    while sum > u16::MAX as u32 {
        sum = (sum & u16::MAX as u32) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(feature = "full-init")]
fn write_ipv4(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>, address: [u8; 4]) {
    for (index, byte) in address.into_iter().enumerate() {
        if index != 0 {
            uart.write(b".");
        }
        let hundreds = byte / 100;
        let tens = (byte / 10) % 10;
        let ones = byte % 10;
        if hundreds != 0 {
            uart.write(&[b'0' + hundreds, b'0' + tens, b'0' + ones]);
        } else if tens != 0 {
            uart.write(&[b'0' + tens, b'0' + ones]);
        } else {
            uart.write(&[b'0' + ones]);
        }
    }
}

#[cfg(all(
    feature = "full-init",
    not(any(feature = "personal", feature = "upstream-supplicant"))
))]
fn write_wifi_error(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    marker: &[u8],
    error: WifiError,
) {
    uart.write(marker);
    uart.write(b":");
    let code = match error {
        WifiError::Runtime(error) => {
            let detail = match error {
                hisi_rf_rtos_driver::Error::NotInstalled => 1,
                hisi_rf_rtos_driver::Error::AlreadyInstalled => 2,
                hisi_rf_rtos_driver::Error::ResourceExhausted => 3,
                hisi_rf_rtos_driver::Error::NoTaskSlots => 4,
                hisi_rf_rtos_driver::Error::InvalidHandle => 5,
                hisi_rf_rtos_driver::Error::InvalidContext => 6,
                hisi_rf_rtos_driver::Error::TimedOut => 7,
                hisi_rf_rtos_driver::Error::Runtime => 8,
            };
            0xffff_ff00 | detail
        }
        WifiError::AlreadyInitialized => 1,
        WifiError::Initialize(code) => code,
        WifiError::Timebase(code) => code,
        WifiError::CreateStation(code)
        | WifiError::RegisterEvents(code)
        | WifiError::OpenStation(code)
        | WifiError::ConfigureSecurity(code)
        | WifiError::StartScan(code) => code as u32,
        #[cfg(feature = "upstream-supplicant")]
        WifiError::SupplicantPort(error) => {
            let detail = match error {
                ws63_rf_rs::UpstreamSupplicantPortError::Runtime(_) => 1,
                ws63_rf_rs::UpstreamSupplicantPortError::InvalidInterfaceName => 2,
                ws63_rf_rs::UpstreamSupplicantPortError::Busy => 3,
                ws63_rf_rs::UpstreamSupplicantPortError::Poisoned => 4,
                ws63_rf_rs::UpstreamSupplicantPortError::InterfaceConflict => 5,
                ws63_rf_rs::UpstreamSupplicantPortError::Abi(_) => 6,
                ws63_rf_rs::UpstreamSupplicantPortError::Rollback { .. } => 7,
            };
            0xffff_fe00 | detail
        }
        WifiError::Busy => 2,
        WifiError::InvalidSsid => 4,
        WifiError::ProtectedNetwork => 5,
        WifiError::OpenNetwork => 6,
        WifiError::UnsupportedSecurity(mode) => mode as u32,
        WifiError::InvalidPassphrase => 7,
        WifiError::CryptoInitialize(code) => code as u32,
        WifiError::Crypto(code) => code,
        WifiError::ScanFailed(status) => match status {
            ws63_rf_rs::wifi::ScanStatus::Success => 0,
            ws63_rf_rs::wifi::ScanStatus::Failed => 1,
            ws63_rf_rs::wifi::ScanStatus::Refused => 2,
            ws63_rf_rs::wifi::ScanStatus::Timeout => 3,
            ws63_rf_rs::wifi::ScanStatus::Unknown(code) => code,
        },
        WifiError::StartConnect(code) | WifiError::StartDisconnect(code) => code as u32,
        WifiError::ConnectFailed(status) | WifiError::Disconnected(status) => status as u32,
        WifiError::Timeout => 3,
        WifiError::UnsupportedTarget => u32::MAX,
    };
    uart.write(b"0x");
    uart.write(&hex8(code));
    uart.write(b"\r\n");
}
