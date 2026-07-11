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

use hisi_panic_handler as _;
use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::delay::Delay;
use hisi_riscv_hal::rf_power::{FactoryXoTrim, RfPower};
use hisi_riscv_hal::system::{ResetReason, System};
use hisi_riscv_hal::uart::{Config, Uart, UartClock};
use hisi_riscv_hal::wdt::Watchdog;
use hisi_riscv_rt::entry;
#[cfg(feature = "full-init")]
use ws63_rf_rs::wifi::{Error as WifiError, MAX_SCAN_RESULTS, ScanResult, Wifi};

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

    #[cfg(feature = "full-init")]
    ws63_rf_rs::set_log_sink(rf_log_uart0);

    uart.write(b"\r\nRF1_IMAGE_OK\r\n");
    run_wifi_smoke(&uart, efuse);

    loop {
        core::hint::spin_loop();
    }
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
        // SAFETY: WS63 trap_entry passes the live 0x90-byte saved-register
        // frame in a0. These offsets mirror startup.S save_all/restore_all.
        let (ra, a0, a1, a2, a3) = unsafe {
            (
                core::ptr::read_volatile(frame.byte_add(0x8c)),
                core::ptr::read_volatile(frame.byte_add(0x7c)),
                core::ptr::read_volatile(frame.byte_add(0x78)),
                core::ptr::read_volatile(frame.byte_add(0x74)),
                core::ptr::read_volatile(frame.byte_add(0x70)),
            )
        };
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
    }
    rf_log_uart0(b"\r\n");
    loop {
        core::hint::spin_loop();
    }
}

#[cfg(not(feature = "full-init"))]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>,
    _efuse: hisi_riscv_hal::peripherals::Efuse<'_>,
) {
    uart.write(b"RF2_INIT_BEGIN\r\n");
    uart.write(b"RF2_INIT_SKIPPED:full-init feature disabled\r\n");
}

#[cfg(feature = "full-init")]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>,
    efuse: hisi_riscv_hal::peripherals::Efuse<'_>,
) {
    uart.write(b"RF2_INIT_BEGIN\r\n");
    let mut wifi = match Wifi::initialize(efuse) {
        Ok(wifi) => wifi,
        Err(error) => {
            write_wifi_error(uart, b"RF2_INIT_ERR", error);
            return;
        }
    };
    uart.write(b"RF2_INIT_OK ifname=");
    uart.write(wifi.interface_name());
    uart.write(b"\r\n");

    uart.write(b"RF3_SCAN_BEGIN\r\n");
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
                uart.write(b"\r\n");
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
}

#[cfg(feature = "full-init")]
fn write_wifi_error(
    uart: &Uart<'_, hisi_riscv_hal::peripherals::Uart0<'_>>,
    marker: &[u8],
    error: WifiError,
) {
    uart.write(marker);
    uart.write(b":");
    let code = match error {
        WifiError::AlreadyInitialized => 1,
        WifiError::Initialize(code) => code,
        WifiError::Timebase(code) => code,
        WifiError::CreateStation(code)
        | WifiError::RegisterEvents(code)
        | WifiError::OpenStation(code)
        | WifiError::StartScan(code) => code as u32,
        WifiError::Busy => 2,
        WifiError::ScanFailed(status) => match status {
            ws63_rf_rs::wifi::ScanStatus::Success => 0,
            ws63_rf_rs::wifi::ScanStatus::Failed => 1,
            ws63_rf_rs::wifi::ScanStatus::Refused => 2,
            ws63_rf_rs::wifi::ScanStatus::Timeout => 3,
            ws63_rf_rs::wifi::ScanStatus::Unknown(code) => code,
        },
        WifiError::Timeout => 3,
        WifiError::UnsupportedTarget => u32::MAX,
    };
    uart.write(b"0x");
    uart.write(&hex8(code));
    uart.write(b"\r\n");
}
