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

use hisi_hal::Peripherals;
use hisi_hal::delay::Delay;
use hisi_hal::rf_power::{FactoryXoTrim, RfPower};
use hisi_hal::system::{ResetReason, System};
use hisi_hal::uart::{Config, Uart, UartClock};
use hisi_hal::wdt::Watchdog;
use hisi_panic_handler as _;
use hisi_riscv_rt::entry;
#[cfg(feature = "full-init")]
use ws63_rf_rs::wifi::{Error as WifiError, MAX_SCAN_RESULTS, ScanResult};
#[cfg(all(feature = "full-init", not(feature = "wpa")))]
use ws63_rf_rs::wifi::{OpenNetwork, Wifi as ActiveWifi};
#[cfg(feature = "wpa")]
use ws63_rf_rs::wifi::{PersonalNetwork, WpaWifi as ActiveWifi};

#[cfg(feature = "full-init")]
const TEST_SSID: &[u8] = b"HUAWEI-HLJ_Guest";
#[cfg(feature = "wpa")]
const TEST_PASSPHRASE: &[u8] = match option_env!("WS63_WIFI_PASSPHRASE") {
    Some(value) => value.as_bytes(),
    None => b"",
};

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
    rf_log_uart0(b"\r\n");
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

#[cfg(feature = "full-init")]
fn run_wifi_smoke(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    efuse: hisi_hal::peripherals::Efuse<'_>,
) {
    uart.write(b"RF2_INIT_BEGIN\r\n");
    let mut wifi = match ActiveWifi::initialize(efuse) {
        Ok(wifi) => wifi,
        Err(error) => {
            write_wifi_error(uart, b"RF2_INIT_ERR", error);
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
            let Some(result) = results[..count]
                .iter()
                .find(|result| result.ssid() == TEST_SSID)
            else {
                uart.write(b"RF5B_AP_NOT_FOUND ssid=");
                uart.write(TEST_SSID);
                uart.write(b"\r\n");
                return;
            };
            #[cfg(not(feature = "wpa"))]
            let network = match OpenNetwork::from_scan(result) {
                Ok(network) => network,
                Err(error) => {
                    write_wifi_error(uart, b"RF5B_CONFIG_ERR", error);
                    return;
                }
            };
            #[cfg(feature = "wpa")]
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
            #[cfg(not(feature = "wpa"))]
            match wifi.connect_open(&network, 15_000) {
                Ok(info) => {
                    uart.write(b"RF5B_CONNECT_OK freq=0x");
                    uart.write(&hex8(info.frequency_mhz as u32));
                    uart.write(b"\r\n");
                    run_arp_probe(uart);
                }
                Err(error) => write_wifi_error(uart, b"RF5B_CONNECT_ERR", error),
            }
            #[cfg(feature = "wpa")]
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
    #[cfg(feature = "rf-init-diag")]
    for irq in [40, 44, 45] {
        uart.write(b"RFDBG_IRQ_COUNT irq=0x");
        uart.write(&hex8(irq));
        uart.write(b" count=0x");
        uart.write(&hex8(ws63_rf_rs::osal::irq_dispatch_count(irq)));
        uart.write(b"\r\n");
    }
}

#[cfg(feature = "full-init")]
fn run_arp_probe(uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>) {
    let Some(mac) = ws63_rf_rs::netif::hardware_address() else {
        uart.write(b"RF5A_ARP_ERR:no-mac\r\n");
        return;
    };
    uart.write(b"RF5A_DHCP_BEGIN\r\n");
    #[cfg(feature = "rf-queue-guard")]
    ws63_rf_rs::netif::arm_host_queue_callback_watchpoint();
    let Some(config) = ws63_rf_rs::netif_smoltcp::dhcp_probe(mac, 10_000) else {
        uart.write(b"RF5A_DHCP_TIMEOUT rx=0x");
        uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
        uart.write(b" tx_failed=0x");
        uart.write(&hex8(ws63_rf_rs::netif::tx_failed()));
        uart.write(b"\r\n");
        return;
    };
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
            run_ping_probe(uart, mac, config.address, gateway, gateway_mac);
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

#[cfg(feature = "full-init")]
fn run_ping_probe(
    uart: &Uart<'_, hisi_hal::peripherals::Uart0<'_>>,
    mac: [u8; 6],
    address: [u8; 4],
    gateway: [u8; 4],
    gateway_mac: [u8; 6],
) {
    const IDENTIFIER: u16 = 0x5753;
    const SEQUENCE: u16 = 1;
    const TARGET: [u8; 4] = [1, 1, 1, 1];
    // Ethernet (14) + IPv4 (20) + ICMP echo header (8) + payload (32).
    let mut request = [0_u8; 74];
    request[..6].copy_from_slice(&gateway_mac);
    request[6..12].copy_from_slice(&mac);
    request[12..14].copy_from_slice(&[0x08, 0x00]);
    request[14] = 0x45;
    let ip_packet_len = (request.len() - 14) as u16;
    request[16..18].copy_from_slice(&ip_packet_len.to_be_bytes());
    request[18..20].copy_from_slice(&1_u16.to_be_bytes());
    request[22] = 64;
    request[23] = 1;
    request[26..30].copy_from_slice(&address);
    request[30..34].copy_from_slice(&TARGET);
    let ip_checksum = internet_checksum(&request[14..34]);
    request[24..26].copy_from_slice(&ip_checksum.to_be_bytes());
    request[34] = 8;
    request[38..40].copy_from_slice(&IDENTIFIER.to_be_bytes());
    request[40..42].copy_from_slice(&SEQUENCE.to_be_bytes());
    for (index, byte) in request[42..].iter_mut().enumerate() {
        *byte = index as u8;
    }
    let icmp_checksum = internet_checksum(&request[34..]);
    request[36..38].copy_from_slice(&icmp_checksum.to_be_bytes());

    uart.write(b"RF5C_PING_BEGIN target=");
    write_ipv4(uart, TARGET);
    uart.write(b" via=");
    write_ipv4(uart, gateway);
    uart.write(b"\r\n");
    if ws63_rf_rs::netif::transmit(&request).is_err() {
        uart.write(b"RF5C_PING_ERR:tx\r\n");
        return;
    }

    let mut frame = [0_u8; ws63_rf_rs::netif_smoltcp::MTU];
    for _ in 0..300 {
        if let Some(length) = ws63_rf_rs::netif_smoltcp::take_received(&mut frame)
            && length >= 42
            && frame[12..14] == [0x08, 0x00]
            && frame[23] == 1
            && frame[26..30] == TARGET
            && frame[30..34] == address
            && frame[34] == 0
            && frame[38..40] == IDENTIFIER.to_be_bytes()
            && frame[40..42] == SEQUENCE.to_be_bytes()
        {
            uart.write(b"RF5C_PING_OK rx=0x");
            uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
            uart.write(b"\r\n");
            return;
        }
        ws63_rf_rs::osal::osal_msleep(10);
    }
    uart.write(b"RF5C_PING_TIMEOUT rx=0x");
    uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
    uart.write(b"\r\n");
}

#[cfg(feature = "full-init")]
fn internet_checksum(bytes: &[u8]) -> u16 {
    let mut sum = 0_u32;
    let mut chunks = bytes.chunks_exact(2);
    for chunk in &mut chunks {
        sum += u16::from_be_bytes([chunk[0], chunk[1]]) as u32;
    }
    if let Some(&last) = chunks.remainder().first() {
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

#[cfg(feature = "full-init")]
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
                hisi_rf_rtos_driver::Error::InvalidHandle => 4,
                hisi_rf_rtos_driver::Error::InvalidContext => 5,
                hisi_rf_rtos_driver::Error::TimedOut => 6,
                hisi_rf_rtos_driver::Error::Runtime => 7,
            };
            0xffff_ff00 | detail
        }
        WifiError::AlreadyInitialized => 1,
        WifiError::Initialize(code) => code,
        WifiError::Timebase(code) => code,
        WifiError::CreateStation(code)
        | WifiError::RegisterEvents(code)
        | WifiError::OpenStation(code)
        | WifiError::StartScan(code) => code as u32,
        WifiError::Busy => 2,
        WifiError::InvalidSsid => 4,
        WifiError::ProtectedNetwork => 5,
        WifiError::OpenNetwork => 6,
        WifiError::UnsupportedSecurity(mode) => mode as u32,
        WifiError::InvalidPassphrase => 7,
        WifiError::Crypto(code) => code,
        WifiError::ScanFailed(status) => match status {
            ws63_rf_rs::wifi::ScanStatus::Success => 0,
            ws63_rf_rs::wifi::ScanStatus::Failed => 1,
            ws63_rf_rs::wifi::ScanStatus::Refused => 2,
            ws63_rf_rs::wifi::ScanStatus::Timeout => 3,
            ws63_rf_rs::wifi::ScanStatus::Unknown(code) => code,
        },
        WifiError::StartConnect(code) => code as u32,
        WifiError::ConnectFailed(status) | WifiError::Disconnected(status) => status as u32,
        WifiError::Timeout => 3,
        WifiError::UnsupportedTarget => u32::MAX,
    };
    uart.write(b"0x");
    uart.write(&hex8(code));
    uart.write(b"\r\n");
}
