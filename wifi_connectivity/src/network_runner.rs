//! Application-owned long-lived IPv4 runner for the connectivity contract.

use embassy_time::{Duration as EmbassyDuration, Timer};
use hisi_rf::WifiController;
use hisi_rf::ws63::{DhcpDiagnostics, WifiDevice};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::socket::{dhcpv4, udp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{EthernetAddress, HardwareAddress, IpAddress, IpCidr, IpEndpoint, Ipv4Address};

use super::config::PUBLIC_DNS_TARGETS;
use super::dns_contract::{ResponseError as DnsResponseError, build_query, validate_response};
use super::{Uart0, halt, hex8, monotonic_ms, write_ipv4};

const DHCP_TIMEOUT_MS: u64 = 30_000;
const DHCP_SMOKE_MAX_LEASE_SECS: u64 = 20;
const DHCP_RECOVERY_RESTART_MS: u64 = 10_000;
const POLL_INTERVAL_MS: u64 = 10;
const DNS_TRANSACTION_BASE: u16 = 0x5753;
const DNS_ATTEMPTS_PER_TARGET: u16 = 2;
const DNS_LOCAL_PORT: u16 = 49_153;
const DNS_PORT: u16 = 53;
const LOCAL_PROBE_PORT: u16 = 9;
const LOCAL_PROBE_PRIMARY_ATTEMPTS: u8 = 5;
const LOCAL_PROBE_ATTEMPTS: u8 = 10;
const LOCAL_PROBE_INTERVAL_MS: u64 = 500;
const LOCAL_PROBE_RECOVERY_DELAY_MS: u64 = 5_000;
const LOCAL_PROBE_TIMEOUT_MS: u64 = 12_000;
const DNS_TIMEOUT_MS: u64 = 1_500;
const DNS_RX_BUFFER_BYTES: usize = 256;
const DNS_TX_BUFFER_BYTES: usize = 32;
const PUBLIC_TARGETS: [Ipv4Address; 2] = [
    Ipv4Address::new(
        PUBLIC_DNS_TARGETS[0][0],
        PUBLIC_DNS_TARGETS[0][1],
        PUBLIC_DNS_TARGETS[0][2],
        PUBLIC_DNS_TARGETS[0][3],
    ),
    Ipv4Address::new(
        PUBLIC_DNS_TARGETS[1][0],
        PUBLIC_DNS_TARGETS[1][1],
        PUBLIC_DNS_TARGETS[1][2],
        PUBLIC_DNS_TARGETS[1][3],
    ),
];

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Lease {
    address: Ipv4Address,
    prefix_len: u8,
    server: Ipv4Address,
    router: Option<Ipv4Address>,
}

#[derive(Clone, Copy, Default)]
struct DnsStats {
    attempts: u32,
    responses: u32,
    invalid: u32,
    tx_errors: u32,
    successful_target: Option<Ipv4Address>,
}

/// Own the L2 device and IP state for the rest of the firmware lifetime.
pub(super) async fn run(uart: &Uart0, controller: &WifiController, device: &mut WifiDevice) -> ! {
    let Some(mac) = device.station_mac_address() else {
        uart.write(b"A4_NET_ERR:no-mac\r\n");
        halt()
    };
    device.reset_rx_queue_diagnostics();
    device.reset_l2_protocol_diagnostics();

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x5753_3633;
    let mut interface = Interface::new(config, device, now());

    let mut socket_storage = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut socket_storage[..]);
    let mut dhcp_socket = dhcpv4::Socket::new();
    // The HIL lease cap is shorter than smoltcp's production-oriented
    // 60-second minimum renew retry, so use a matching bounded retry profile.
    let mut dhcp_retry = dhcpv4::RetryConfig::default();
    dhcp_retry.discover_timeout = Duration::from_secs(2);
    dhcp_retry.initial_request_timeout = Duration::from_secs(2);
    dhcp_retry.min_renew_timeout = Duration::from_secs(2);
    dhcp_retry.max_renew_timeout = Duration::from_secs(5);
    dhcp_socket.set_retry_config(dhcp_retry);
    dhcp_socket.set_max_lease_duration(Some(Duration::from_secs(DHCP_SMOKE_MAX_LEASE_SECS)));
    let dhcp_handle = sockets.add(dhcp_socket);

    let mut dns_rx_metadata = [udp::PacketMetadata::EMPTY; 1];
    let mut dns_tx_metadata = [udp::PacketMetadata::EMPTY; 1];
    let mut dns_rx_storage = [0_u8; DNS_RX_BUFFER_BYTES];
    let mut dns_tx_storage = [0_u8; DNS_TX_BUFFER_BYTES];
    let dns_rx = udp::PacketBuffer::new(&mut dns_rx_metadata[..], &mut dns_rx_storage[..]);
    let dns_tx = udp::PacketBuffer::new(&mut dns_tx_metadata[..], &mut dns_tx_storage[..]);
    let dns_handle = sockets.add(udp::Socket::new(dns_rx, dns_tx));
    sockets
        .get_mut::<udp::Socket>(dns_handle)
        .bind(DNS_LOCAL_PORT)
        .expect("bind DNS probe socket");

    uart.write(b"A4_NET_RUNNER_BEGIN stack=smoltcp\r\n");
    uart.write(b"RF5A_DHCP_BEGIN\r\n");
    let started_at = monotonic_ms();
    let mut lease = None;
    while lease.is_none() && monotonic_ms().wrapping_sub(started_at) < DHCP_TIMEOUT_MS {
        poll_network(
            uart,
            &mut interface,
            device,
            &mut sockets,
            dhcp_handle,
            &mut lease,
        );
        sleep_poll_interval().await;
    }

    let Some(active_lease) = lease else {
        let diagnostics = hisi_rf::ws63::diagnostics(controller, device);
        let queue = diagnostics.rx_queue;
        let dhcp = diagnostics.dhcp;
        let data_path = diagnostics.data_path;
        uart.write(b"RF5A_DHCP_TIMEOUT rx_drop=0x");
        uart.write(&hex8(queue.dropped));
        uart.write(b" client=0x");
        uart.write(&hex8(dhcp.client_packets));
        uart.write(b" server=0x");
        uart.write(&hex8(dhcp.server_packets));
        uart.write(b"\r\nRFDBG_A5B_DHCP_TIMEOUT_PATH dmac_rx=0x");
        uart.write(&hex8(data_path.dmac_rx_prepares));
        uart.write(b" hmac_event=0x");
        uart.write(&hex8(data_path.hmac_rx_data_event_adapt_calls));
        uart.write(b" hmac_msg=0x");
        uart.write(&hex8(data_path.hmac_rx_process_data_msg_calls));
        uart.write(b" hmac_data=0x");
        uart.write(&hex8(data_path.hmac_rx_data_calls));
        uart.write(b" vendor_rx=0x");
        uart.write(&hex8(data_path.vendor_rx_frames));
        uart.write(b" mac_rx_ok=0x");
        uart.write(&hex8(data_path.mac_rx_successful_mpdu));
        uart.write(b" mac_rx_fail=0x");
        uart.write(&hex8(data_path.mac_rx_failed_mpdu));
        uart.write(b" mac_rx_filter=0x");
        uart.write(&hex8(data_path.mac_rx_filtered_mpdu));
        uart.write(b" ccmp_replay=0x");
        uart.write(&hex8(data_path.mac_ccmp_replay_failures));
        uart.write(b" tkip_replay=0x");
        uart.write(&hex8(data_path.mac_tkip_replay_failures));
        uart.write(b" ccmp_mic=0x");
        uart.write(&hex8(data_path.mac_ccmp_mic_failures));
        uart.write(b" tkip_mic=0x");
        uart.write(&hex8(data_path.mac_tkip_mic_failures));
        uart.write(b" key_search_fail=0x");
        uart.write(&hex8(data_path.mac_key_search_failures));
        uart.write(b" irq45=0x");
        uart.write(&hex8(data_path.wlmac_irqs));
        uart.write(b"\r\nA4_NET_ERR:dhcp-timeout\r\n");
        halt()
    };
    let dhcp_baseline = device.dhcp_diagnostics();

    let local_target = active_lease.router.unwrap_or(active_lease.server);
    let local_echo = local_neighbor_probe(
        uart,
        &mut interface,
        device,
        &mut sockets,
        dhcp_handle,
        dns_handle,
        &mut lease,
        local_target,
    )
    .await;
    let dns_stats = if active_lease.router.is_some() {
        dns_probe(
            uart,
            &mut interface,
            device,
            &mut sockets,
            dhcp_handle,
            dns_handle,
            &mut lease,
            &PUBLIC_TARGETS,
        )
        .await
    } else {
        uart.write(b"RF5C_PUBLIC_DNS_SKIP reason=no-default-route\r\n");
        DnsStats::default()
    };
    let l2_gate = device.l2_protocol_diagnostics();

    if l2_gate.rx_arp_replies != 0 && local_echo {
        uart.write(b"RF5A_ARP_OK evidence=l2-arp-reply\r\n");
        uart.write(b"RF5C_LOCAL_DATA_PATH_OK echo=ok arp_reply=0x");
    } else {
        uart.write(b"RF5C_LOCAL_DATA_PATH_ERR echo=");
        if local_echo {
            uart.write(b"ok");
        } else {
            uart.write(b"missing");
        }
        uart.write(b" arp_reply=0x");
    }
    uart.write(&hex8(l2_gate.rx_arp_replies));
    uart.write(b" arp_request=0x");
    uart.write(&hex8(l2_gate.tx_arp_requests));
    uart.write(b" gateway=");
    if let Some(router) = active_lease.router {
        write_ipv4(uart, router.octets());
    } else {
        uart.write(b"none");
    }
    uart.write(b"\r\n");
    if active_lease.router.is_some() {
        if dns_stats.responses == 0 {
            uart.write(b"RF5C_PUBLIC_DNS_ERR target=");
        } else {
            uart.write(b"RF5C_PUBLIC_DNS_OK target=");
        }
        if let Some(target) = dns_stats.successful_target {
            write_ipv4(uart, target.octets());
        } else {
            uart.write(b"none");
        }
        uart.write(b" attempts=0x");
        uart.write(&hex8(dns_stats.attempts));
        uart.write(b" responses=0x");
        uart.write(&hex8(dns_stats.responses));
        uart.write(b" invalid=0x");
        uart.write(&hex8(dns_stats.invalid));
        uart.write(b" tx_error=0x");
        uart.write(&hex8(dns_stats.tx_errors));
        uart.write(b"\r\n");
    }

    let diagnostics = hisi_rf::ws63::diagnostics(controller, device);
    let queue = diagnostics.rx_queue;
    let dhcp = diagnostics.dhcp;
    let l2 = diagnostics.l2_protocol;
    let data_path = diagnostics.data_path;
    let mut last_tx = [0_u8; 64];
    let last_tx_len = device.last_transmitted_frame(&mut last_tx);
    let mut last_rx = [0_u8; 64];
    let last_rx_len = device.last_received_frame(&mut last_rx);
    uart.write(b"RF5C_CONNECTIVITY_SUMMARY arp_request=0x");
    uart.write(&hex8(l2.tx_arp_requests));
    uart.write(b" arp_reply=0x");
    uart.write(&hex8(l2.rx_arp_replies));
    uart.write(b" dns_attempts=0x");
    uart.write(&hex8(dns_stats.attempts));
    uart.write(b" dns_responses=0x");
    uart.write(&hex8(dns_stats.responses));
    uart.write(b" dns_invalid=0x");
    uart.write(&hex8(dns_stats.invalid));
    uart.write(b" dns_tx_error=0x");
    uart.write(&hex8(dns_stats.tx_errors));
    uart.write(b" rx_queue_drop=0x");
    uart.write(&hex8(queue.dropped));
    uart.write(b"\r\n");
    uart.write(b"RFDBG_A5B_L2 rx_arp_req=0x");
    uart.write(&hex8(l2.rx_arp_requests));
    uart.write(b" rx_arp_reply=0x");
    uart.write(&hex8(l2.rx_arp_replies));
    uart.write(b" rx_ipv4=0x");
    uart.write(&hex8(l2.rx_ipv4));
    uart.write(b" rx_other=0x");
    uart.write(&hex8(l2.rx_other));
    uart.write(b" tx_arp_req=0x");
    uart.write(&hex8(l2.tx_arp_requests));
    uart.write(b" tx_arp_reply=0x");
    uart.write(&hex8(l2.tx_arp_replies));
    uart.write(b" tx_ipv4=0x");
    uart.write(&hex8(l2.tx_ipv4));
    uart.write(b" tx_other=0x");
    uart.write(&hex8(l2.tx_other));
    uart.write(b"\r\n");
    uart.write(b"RFDBG_A5B_DATA_PATH tx=0x");
    uart.write(&hex8(data_path.tx_frames));
    uart.write(b" path_caps=0x");
    uart.write(&hex8(data_path.instrumented_capabilities));
    uart.write(b" tx_failed=0x");
    uart.write(&hex8(data_path.tx_failed));
    uart.write(b" vendor_tx=0x");
    uart.write(&hex8(data_path.vendor_tx_frames));
    uart.write(b" tx_complete=0x");
    uart.write(&hex8(data_path.tx_completions));
    uart.write(b" dmac_rx=0x");
    uart.write(&hex8(data_path.dmac_rx_prepares));
    uart.write(b" dmac_zero=0x");
    uart.write(&hex8(data_path.dmac_rx_prepare_zero));
    uart.write(b" dmac_nonzero=0x");
    uart.write(&hex8(data_path.dmac_rx_prepare_nonzero));
    uart.write(b" dmac_last=0x");
    uart.write(&hex8(data_path.dmac_rx_prepare_last_result));
    uart.write(b" hmac_event=0x");
    uart.write(&hex8(data_path.hmac_rx_data_event_adapt_calls));
    uart.write(b" hmac_msg=0x");
    uart.write(&hex8(data_path.hmac_rx_process_data_msg_calls));
    uart.write(b" hmac_data=0x");
    uart.write(&hex8(data_path.hmac_rx_data_calls));
    uart.write(b" vendor_rx=0x");
    uart.write(&hex8(data_path.vendor_rx_frames));
    uart.write(b" rx=0x");
    uart.write(&hex8(data_path.rx_frames));
    uart.write(b" mac_rx_ok=0x");
    uart.write(&hex8(data_path.mac_rx_successful_mpdu));
    uart.write(b" mac_rx_fail=0x");
    uart.write(&hex8(data_path.mac_rx_failed_mpdu));
    uart.write(b" mac_rx_filter=0x");
    uart.write(&hex8(data_path.mac_rx_filtered_mpdu));
    uart.write(b" ccmp_replay=0x");
    uart.write(&hex8(data_path.mac_ccmp_replay_failures));
    uart.write(b" tkip_replay=0x");
    uart.write(&hex8(data_path.mac_tkip_replay_failures));
    uart.write(b" ccmp_mic=0x");
    uart.write(&hex8(data_path.mac_ccmp_mic_failures));
    uart.write(b" tkip_mic=0x");
    uart.write(&hex8(data_path.mac_tkip_mic_failures));
    uart.write(b" key_search_fail=0x");
    uart.write(&hex8(data_path.mac_key_search_failures));
    uart.write(b" rx_filter_ctl=0x");
    uart.write(&hex8(data_path.mac_rx_filter_control));
    uart.write(b" sta_addr_match=0x");
    uart.write(&hex8(u32::from(
        data_path.mac_station_address_matches_device,
    )));
    uart.write(b" bssid_programmed=0x");
    uart.write(&hex8(u32::from(data_path.mac_bssid_programmed)));
    uart.write(b" rx_pending=0x");
    uart.write(&hex8(queue.pending.min(u32::MAX as usize) as u32));
    uart.write(b" rx_high_water=0x");
    uart.write(&hex8(queue.high_watermark.min(u32::MAX as usize) as u32));
    uart.write(b" icmp_rx=0x");
    uart.write(&hex8(queue.icmp_echo_replies));
    uart.write(b" icmp_mask=0x");
    uart.write(&hex8(queue.icmp_sequence_mask));
    uart.write(b" dhcp_tx=0x");
    uart.write(&hex8(dhcp.client_packets));
    uart.write(b" dhcp_rx=0x");
    uart.write(&hex8(dhcp.server_packets));
    uart.write(b" irq40=0x");
    uart.write(&hex8(data_path.coex_wlan_irqs));
    uart.write(b" irq44=0x");
    uart.write(&hex8(data_path.wlphy_irqs));
    uart.write(b" irq45=0x");
    uart.write(&hex8(data_path.wlmac_irqs));
    uart.write(b" last_tx_len=0x");
    uart.write(&hex8(last_tx_len.min(u32::MAX as usize) as u32));
    if last_tx_len >= 14 {
        uart.write(b" last_tx_dst_hi=0x");
        uart.write(&hex8(u32::from_be_bytes([
            last_tx[0], last_tx[1], last_tx[2], last_tx[3],
        ])));
        uart.write(b" last_tx_dst_lo=0x");
        uart.write(&hex8(u32::from(u16::from_be_bytes([
            last_tx[4], last_tx[5],
        ]))));
        uart.write(b" last_tx_ethertype=0x");
        uart.write(&hex8(u32::from(u16::from_be_bytes([
            last_tx[12],
            last_tx[13],
        ]))));
    }
    uart.write(b" last_rx_len=0x");
    uart.write(&hex8(last_rx_len.min(u32::MAX as usize) as u32));
    if last_rx_len >= 14 {
        uart.write(b" last_rx_src_hi=0x");
        uart.write(&hex8(u32::from_be_bytes([
            last_rx[6], last_rx[7], last_rx[8], last_rx[9],
        ])));
        uart.write(b" last_rx_src_lo=0x");
        uart.write(&hex8(u32::from(u16::from_be_bytes([
            last_rx[10],
            last_rx[11],
        ]))));
        uart.write(b" last_rx_ethertype=0x");
        uart.write(&hex8(u32::from(u16::from_be_bytes([
            last_rx[12],
            last_rx[13],
        ]))));
    }
    if last_rx_len >= 42 && last_rx[12..14] == [0x08, 0x00] && last_rx[23] == 17 {
        let udp = 14 + usize::from(last_rx[14] & 0x0f) * 4;
        if udp + 8 <= last_rx.len() {
            uart.write(b" last_rx_ip_len=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[16],
                last_rx[17],
            ]))));
            uart.write(b" last_rx_ip_sum=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[24],
                last_rx[25],
            ]))));
            uart.write(b" last_rx_udp_src=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[udp],
                last_rx[udp + 1],
            ]))));
            uart.write(b" last_rx_udp_dst=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[udp + 2],
                last_rx[udp + 3],
            ]))));
            uart.write(b" last_rx_udp_len=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[udp + 4],
                last_rx[udp + 5],
            ]))));
            uart.write(b" last_rx_udp_sum=0x");
            uart.write(&hex8(u32::from(u16::from_be_bytes([
                last_rx[udp + 6],
                last_rx[udp + 7],
            ]))));
        }
    }
    uart.write(b"\r\n");
    super::write_rtos_task_diagnostics(uart);
    uart.write(b"A4_NET_RUNNER_STEADY lease=managed neighbor_cache=managed\r\n");
    uart.flush_tx();

    keep_polling(
        uart,
        controller,
        &mut interface,
        device,
        &mut sockets,
        dhcp_handle,
        &mut lease,
        dhcp_baseline,
    )
    .await
}

async fn keep_polling(
    uart: &Uart0,
    controller: &WifiController,
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    lease: &mut Option<Lease>,
    dhcp_baseline: DhcpDiagnostics,
) -> ! {
    let mut heartbeat_at = monotonic_ms().saturating_add(10_000);
    let mut renew_reported = false;
    let mut dhcp_restart_at = None;
    loop {
        let had_lease = lease.is_some();
        poll_network(uart, interface, device, sockets, dhcp_handle, lease);
        if had_lease && lease.is_none() {
            write_lease_loss_diagnostics(uart, controller, device, dhcp_baseline);
        }
        let current = monotonic_ms();
        if lease.is_some() {
            dhcp_restart_at = None;
        } else if let Some(restart_at) = dhcp_restart_at {
            if current >= restart_at {
                sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).reset();
                uart.write(b"A4_DHCP_RESTART reason=lease-down-timeout\r\n");
                dhcp_restart_at = Some(current.saturating_add(DHCP_RECOVERY_RESTART_MS));
            }
        } else {
            dhcp_restart_at = Some(current.saturating_add(DHCP_RECOVERY_RESTART_MS));
        }
        let dhcp = device.dhcp_diagnostics();
        if !renew_reported
            && dhcp.client_packets > dhcp_baseline.client_packets
            && dhcp.server_packets > dhcp_baseline.server_packets
        {
            uart.write(b"A4_DHCP_RENEW_OK client=0x");
            uart.write(&hex8(dhcp.client_packets - dhcp_baseline.client_packets));
            uart.write(b" server=0x");
            uart.write(&hex8(dhcp.server_packets - dhcp_baseline.server_packets));
            uart.write(b"\r\n");
            super::write_heap_diagnostics(uart);
            renew_reported = true;
        }
        if current >= heartbeat_at {
            uart.write(b"A4_NET_RUNNER_ALIVE lease=");
            uart.write(if lease.is_some() {
                b"up\r\n"
            } else {
                b"down\r\n"
            });
            heartbeat_at = current.saturating_add(10_000);
        }
        sleep_poll_interval().await;
    }
}

fn write_lease_loss_diagnostics(
    uart: &Uart0,
    controller: &WifiController,
    device: &WifiDevice,
    dhcp_baseline: DhcpDiagnostics,
) {
    let diagnostics = hisi_rf::ws63::diagnostics(controller, device);
    let dhcp = diagnostics.dhcp;
    let data_path = diagnostics.data_path;
    uart.write(b"RFDBG_A4_DHCP_LEASE_LOSS dhcp_tx_delta=0x");
    uart.write(&hex8(
        dhcp.client_packets
            .saturating_sub(dhcp_baseline.client_packets),
    ));
    uart.write(b" dhcp_rx_delta=0x");
    uart.write(&hex8(
        dhcp.server_packets
            .saturating_sub(dhcp_baseline.server_packets),
    ));
    uart.write(b" dmac_rx=0x");
    uart.write(&hex8(data_path.dmac_rx_prepares));
    uart.write(b" hmac_event=0x");
    uart.write(&hex8(data_path.hmac_rx_data_event_adapt_calls));
    uart.write(b" hmac_msg=0x");
    uart.write(&hex8(data_path.hmac_rx_process_data_msg_calls));
    uart.write(b" hmac_data=0x");
    uart.write(&hex8(data_path.hmac_rx_data_calls));
    uart.write(b" vendor_rx=0x");
    uart.write(&hex8(data_path.vendor_rx_frames));
    uart.write(b" ccmp_replay=0x");
    uart.write(&hex8(data_path.mac_ccmp_replay_failures));
    uart.write(b" ccmp_mic=0x");
    uart.write(&hex8(data_path.mac_ccmp_mic_failures));
    uart.write(b" key_search_fail=0x");
    uart.write(&hex8(data_path.mac_key_search_failures));
    uart.write(b" irq45=0x");
    uart.write(&hex8(data_path.wlmac_irqs));
    uart.write(b"\r\n");
}

fn poll_network(
    uart: &Uart0,
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    lease: &mut Option<Lease>,
) {
    let _ = interface.poll(now(), device, sockets);
    let dhcp = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle);
    match dhcp.poll() {
        Some(dhcpv4::Event::Configured(config)) => {
            let next = Lease {
                address: config.address.address(),
                prefix_len: config.address.prefix_len(),
                server: config.server.address,
                router: config.router,
            };
            interface.update_ip_addrs(|addresses| {
                addresses.clear();
                addresses
                    .push(IpCidr::Ipv4(config.address))
                    .expect("one IPv4 address fits");
            });
            if let Some(router) = config.router {
                interface
                    .routes_mut()
                    .add_default_ipv4_route(router)
                    .expect("one default route fits");
            } else {
                interface.routes_mut().remove_default_ipv4_route();
            }
            if lease.is_none() {
                uart.write(b"RF5A_DHCP_OK addr=");
                write_ipv4(uart, next.address.octets());
                uart.write(b" prefix=0x");
                uart.write(&hex8(u32::from(next.prefix_len)));
                uart.write(b" router=");
                if let Some(router) = next.router {
                    write_ipv4(uart, router.octets());
                } else {
                    uart.write(b"none");
                }
                uart.write(b"\r\n");
            } else {
                uart.write(b"A4_DHCP_RENEWED\r\n");
            }
            *lease = Some(next);
        }
        Some(dhcpv4::Event::Deconfigured) => {
            let had_lease = lease.take().is_some();
            interface.update_ip_addrs(|addresses| addresses.clear());
            interface.routes_mut().remove_default_ipv4_route();
            uart.write(b"A4_DHCP_DECONFIGURED\r\n");
            if had_lease {
                // A lease expiry already transitions smoltcp back toward
                // discovery, but an explicit reset also clears any stale
                // renewal/rebinding schedule and makes the next DISCOVER
                // immediately eligible.
                dhcp.reset();
                uart.write(b"A4_DHCP_RESTART reason=lease-lost\r\n");
            }
        }
        None => {}
    }
}

#[allow(clippy::too_many_arguments)]
async fn local_neighbor_probe(
    uart: &Uart0,
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    udp_handle: SocketHandle,
    lease: &mut Option<Lease>,
    target: Ipv4Address,
) -> bool {
    uart.write(b"RF5C_LOCAL_NEIGHBOR_BEGIN target=");
    write_ipv4(uart, target.octets());
    uart.write(b"\r\n");

    let endpoint = IpEndpoint::new(IpAddress::Ipv4(target), LOCAL_PROBE_PORT);
    let started_at = monotonic_ms();
    let mut sent = 0_u8;
    let mut last_send_at = started_at.wrapping_sub(LOCAL_PROBE_INTERVAL_MS);
    let mut recovery_announced = false;
    while monotonic_ms().wrapping_sub(started_at) < LOCAL_PROBE_TIMEOUT_MS {
        poll_network(uart, interface, device, sockets, dhcp_handle, lease);
        let socket = sockets.get_mut::<udp::Socket>(udp_handle);
        while socket.can_recv() {
            let Ok((response, metadata)) = socket.recv() else {
                break;
            };
            if metadata.endpoint == endpoint && response.len() == 1 && response[0] < sent {
                uart.write(b"RF5C_LOCAL_ECHO_OK target=");
                write_ipv4(uart, target.octets());
                uart.write(b" attempts=0x");
                uart.write(&hex8(u32::from(sent)));
                uart.write(b" sequence=0x");
                uart.write(&hex8(u32::from(response[0])));
                uart.write(b"\r\n");
                return true;
            }
        }
        let current_ms = monotonic_ms();
        let recovery_ready = sent < LOCAL_PROBE_PRIMARY_ATTEMPTS
            || current_ms.wrapping_sub(started_at) >= LOCAL_PROBE_RECOVERY_DELAY_MS;
        if sent == LOCAL_PROBE_PRIMARY_ATTEMPTS && recovery_ready && !recovery_announced {
            uart.write(b"RF5C_LOCAL_ECHO_RECOVERY_BEGIN elapsed_ms=0x");
            uart.write(&hex8(
                current_ms.wrapping_sub(started_at).min(u64::from(u32::MAX)) as u32,
            ));
            uart.write(b"\r\n");
            recovery_announced = true;
        }
        if sent < LOCAL_PROBE_ATTEMPTS
            && recovery_ready
            && current_ms.wrapping_sub(last_send_at) >= LOCAL_PROBE_INTERVAL_MS
        {
            let payload = [sent];
            if socket.send_slice(&payload, endpoint).is_ok() {
                sent = sent.saturating_add(1);
                last_send_at = current_ms;
            }
        }
        sleep_poll_interval().await;
    }
    uart.write(b"RF5C_LOCAL_ECHO_ERR target=");
    write_ipv4(uart, target.octets());
    uart.write(b" attempts=0x");
    uart.write(&hex8(u32::from(sent)));
    uart.write(b"\r\n");
    false
}

#[allow(clippy::too_many_arguments)]
async fn dns_probe(
    uart: &Uart0,
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    dns_handle: SocketHandle,
    lease: &mut Option<Lease>,
    targets: &[Ipv4Address],
) -> DnsStats {
    let mut stats = DnsStats::default();
    let total_attempts = DNS_ATTEMPTS_PER_TARGET.saturating_mul(targets.len() as u16);

    uart.write(b"RF5C_PUBLIC_DNS_BEGIN primary=");
    write_ipv4(uart, targets[0].octets());
    uart.write(b" secondary=");
    write_ipv4(uart, targets[1].octets());
    uart.write(b" attempts=0x");
    uart.write(&hex8(u32::from(total_attempts)));
    uart.write(b"\r\n");

    for attempt in 1..=total_attempts {
        let target = targets[usize::from(attempt - 1) % targets.len()];
        let remote = IpEndpoint::new(IpAddress::Ipv4(target), DNS_PORT);
        let transaction_id = DNS_TRANSACTION_BASE.wrapping_add(attempt);
        let query = build_query(transaction_id);
        let sent = sockets
            .get_mut::<udp::Socket>(dns_handle)
            .send_slice(&query, remote);
        stats.attempts = stats.attempts.saturating_add(1);
        if sent.is_err() {
            stats.tx_errors = stats.tx_errors.saturating_add(1);
            write_dns_sample_prefix(uart, attempt, transaction_id, target);
            uart.write(b" status=tx_error\r\n");
            continue;
        }

        let started_at = monotonic_ms();
        let mut accepted = false;
        while monotonic_ms().wrapping_sub(started_at) < DNS_TIMEOUT_MS {
            poll_network(uart, interface, device, sockets, dhcp_handle, lease);
            let socket = sockets.get_mut::<udp::Socket>(dns_handle);
            while socket.can_recv() {
                let Ok((response, metadata)) = socket.recv() else {
                    break;
                };
                if metadata.endpoint != remote {
                    stats.invalid = stats.invalid.saturating_add(1);
                    continue;
                }
                match validate_response(response, transaction_id) {
                    Ok(valid) => {
                        stats.responses = stats.responses.saturating_add(1);
                        stats.successful_target = Some(target);
                        write_dns_sample_prefix(uart, attempt, transaction_id, target);
                        uart.write(b" status=ok answers=0x");
                        uart.write(&hex8(u32::from(valid.answer_count)));
                        uart.write(b"\r\n");
                        accepted = true;
                        break;
                    }
                    Err(error) => {
                        stats.invalid = stats.invalid.saturating_add(1);
                        write_dns_sample_prefix(uart, attempt, transaction_id, target);
                        uart.write(b" status=invalid reason=");
                        uart.write(dns_error_name(error));
                        uart.write(b"\r\n");
                    }
                }
            }
            if accepted {
                break;
            }
            sleep_poll_interval().await;
        }
        if accepted {
            break;
        }
        write_dns_sample_prefix(uart, attempt, transaction_id, target);
        uart.write(b" status=timeout\r\n");
    }
    stats
}

fn write_dns_sample_prefix(uart: &Uart0, attempt: u16, transaction_id: u16, target: Ipv4Address) {
    uart.write(b"RF5C_PUBLIC_DNS_SAMPLE attempt=0x");
    uart.write(&hex8(u32::from(attempt)));
    uart.write(b" txid=0x");
    uart.write(&hex8(u32::from(transaction_id)));
    uart.write(b" target=");
    write_ipv4(uart, target.octets());
}

fn dns_error_name(error: DnsResponseError) -> &'static [u8] {
    match error {
        DnsResponseError::Truncated => b"truncated",
        DnsResponseError::TransactionId => b"transaction-id",
        DnsResponseError::NotResponse => b"not-response",
        DnsResponseError::Opcode => b"opcode",
        DnsResponseError::TruncatedResponse => b"truncated-response",
        DnsResponseError::ResponseCode => b"response-code",
        DnsResponseError::QuestionCount => b"question-count",
        DnsResponseError::Question => b"question",
        DnsResponseError::NoAnswers => b"no-answers",
    }
}

fn now() -> Instant {
    Instant::from_millis(monotonic_ms().min(i64::MAX as u64) as i64)
}

async fn sleep_poll_interval() {
    Timer::after(EmbassyDuration::from_millis(POLL_INTERVAL_MS)).await;
}
