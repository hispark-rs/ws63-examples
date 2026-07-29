//! Application-owned long-lived IPv4 runner for the connectivity contract.

use embassy_time::{Duration as EmbassyDuration, Timer};
use hisi_rf::ws63::{DhcpDiagnostics, WifiDevice};
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{ChecksumCapabilities, Device};
use smoltcp::socket::{dhcpv4, icmp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address,
};

use super::{Uart0, halt, hex8, monotonic_ms, write_ipv4};

const DHCP_TIMEOUT_MS: u64 = 30_000;
const DHCP_SMOKE_MAX_LEASE_SECS: u64 = 20;
const POLL_INTERVAL_MS: u64 = 10;
const PING_TIMEOUT_MS: u64 = 1_000;
const PING_COUNT: u16 = 5;
const ICMP_IDENTIFIER: u16 = 0x5753;
// The connectivity probe sends a 32-byte payload plus the 8-byte ICMP header.
// Keep bounded headroom without spending a full KiB of the calibrated WS63
// SRAM envelope on the two packet queues.
const ICMP_PACKET_BUFFER_BYTES: usize = 128;
const PUBLIC_TARGET: Ipv4Address = Ipv4Address::new(1, 1, 1, 1);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct Lease {
    address: Ipv4Address,
    prefix_len: u8,
    router: Option<Ipv4Address>,
}

#[derive(Clone, Copy, Default)]
struct PingStats {
    tx: u32,
    rx: u32,
    tx_errors: u32,
    rtt_total_ms: u64,
    rtt_min_ms: u32,
    rtt_max_ms: u32,
}

/// Own the L2 device and IP state for the rest of the firmware lifetime.
pub(super) async fn run(uart: &Uart0, device: &mut WifiDevice) -> ! {
    let Some(mac) = device.station_mac_address() else {
        uart.write(b"A4_NET_ERR:no-mac\r\n");
        halt()
    };
    device.reset_rx_queue_diagnostics();

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

    let mut icmp_rx_metadata = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_tx_metadata = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_rx_storage = [0_u8; ICMP_PACKET_BUFFER_BYTES];
    let mut icmp_tx_storage = [0_u8; ICMP_PACKET_BUFFER_BYTES];
    let icmp_rx = icmp::PacketBuffer::new(&mut icmp_rx_metadata[..], &mut icmp_rx_storage[..]);
    let icmp_tx = icmp::PacketBuffer::new(&mut icmp_tx_metadata[..], &mut icmp_tx_storage[..]);
    let icmp_handle = sockets.add(icmp::Socket::new(icmp_rx, icmp_tx));
    sockets
        .get_mut::<icmp::Socket>(icmp_handle)
        .bind(icmp::Endpoint::Ident(ICMP_IDENTIFIER))
        .expect("bind ICMP echo socket");

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
        let queue = device.rx_queue_diagnostics();
        let dhcp = device.dhcp_diagnostics();
        uart.write(b"RF5A_DHCP_TIMEOUT rx_drop=0x");
        uart.write(&hex8(queue.dropped));
        uart.write(b" client=0x");
        uart.write(&hex8(dhcp.client_packets));
        uart.write(b" server=0x");
        uart.write(&hex8(dhcp.server_packets));
        uart.write(b"\r\nA4_NET_ERR:dhcp-timeout\r\n");
        halt()
    };
    let dhcp_baseline = device.dhcp_diagnostics();

    let mut neighbor_confirmed = false;
    let gateway_stats = if let Some(gateway) = active_lease.router {
        uart.write(b"RF5A_ARP_BEGIN target=");
        write_ipv4(uart, gateway.octets());
        uart.write(b" mode=smoltcp\r\n");
        ping_series(
            uart,
            &mut interface,
            device,
            &mut sockets,
            dhcp_handle,
            icmp_handle,
            &mut lease,
            &mut neighbor_confirmed,
            gateway,
            PING_COUNT,
        )
        .await
    } else {
        PingStats::default()
    };
    let public_stats = ping_series(
        uart,
        &mut interface,
        device,
        &mut sockets,
        dhcp_handle,
        icmp_handle,
        &mut lease,
        &mut neighbor_confirmed,
        PUBLIC_TARGET,
        PING_COUNT,
    )
    .await;

    let queue = device.rx_queue_diagnostics();
    uart.write(b"RF5C_CONNECTIVITY_SUMMARY gateway_tx=0x");
    uart.write(&hex8(gateway_stats.tx));
    uart.write(b" gateway_rx=0x");
    uart.write(&hex8(gateway_stats.rx));
    uart.write(b" public_tx=0x");
    uart.write(&hex8(public_stats.tx));
    uart.write(b" public_rx=0x");
    uart.write(&hex8(public_stats.rx));
    uart.write(b" rx_queue_drop=0x");
    uart.write(&hex8(queue.dropped));
    uart.write(b"\r\n");
    uart.write(b"A4_NET_RUNNER_STEADY lease=managed neighbor_cache=managed\r\n");

    keep_polling(
        uart,
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
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    lease: &mut Option<Lease>,
    dhcp_baseline: DhcpDiagnostics,
) -> ! {
    let mut heartbeat_at = monotonic_ms().saturating_add(10_000);
    let mut renew_reported = false;
    loop {
        poll_network(uart, interface, device, sockets, dhcp_handle, lease);
        let current = monotonic_ms();
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
async fn ping_series(
    uart: &Uart0,
    interface: &mut Interface,
    device: &mut WifiDevice,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    icmp_handle: SocketHandle,
    lease: &mut Option<Lease>,
    neighbor_confirmed: &mut bool,
    target: Ipv4Address,
    count: u16,
) -> PingStats {
    let checksum = device.capabilities().checksum;
    let mut stats = PingStats::default();
    let mut payload = [0_u8; 32];

    uart.write(b"RF5C_PING_SERIES_BEGIN target=");
    write_ipv4(uart, target.octets());
    uart.write(b" count=0x");
    uart.write(&hex8(u32::from(count)));
    uart.write(b"\r\n");

    for sequence in 1..=count {
        let started_at = monotonic_ms();
        payload[..8].copy_from_slice(&started_at.to_le_bytes());
        payload[8..10].copy_from_slice(&sequence.to_le_bytes());
        let repr = Icmpv4Repr::EchoRequest {
            ident: ICMP_IDENTIFIER,
            seq_no: sequence,
            data: &payload,
        };
        let sent = {
            let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
            socket
                .send(repr.buffer_len(), IpAddress::Ipv4(target))
                .map(|buffer| {
                    let mut packet = Icmpv4Packet::new_unchecked(buffer);
                    repr.emit(&mut packet, &checksum);
                })
        };

        uart.write(b"RF5C_PING_SAMPLE target=");
        write_ipv4(uart, target.octets());
        uart.write(b" seq=0x");
        uart.write(&hex8(u32::from(sequence)));
        if sent.is_err() {
            stats.tx_errors = stats.tx_errors.saturating_add(1);
            uart.write(b" status=tx_error\r\n");
            continue;
        }
        stats.tx = stats.tx.saturating_add(1);

        let mut received = false;
        while monotonic_ms().wrapping_sub(started_at) < PING_TIMEOUT_MS {
            poll_network(uart, interface, device, sockets, dhcp_handle, lease);
            let socket = sockets.get_mut::<icmp::Socket>(icmp_handle);
            while socket.can_recv() {
                let Ok((bytes, endpoint)) = socket.recv() else {
                    break;
                };
                let Ok(packet) = Icmpv4Packet::new_checked(bytes) else {
                    continue;
                };
                let Ok(Icmpv4Repr::EchoReply {
                    ident,
                    seq_no,
                    data: _,
                }) = Icmpv4Repr::parse(&packet, &ChecksumCapabilities::default())
                else {
                    continue;
                };
                if endpoint == IpAddress::Ipv4(target)
                    && ident == ICMP_IDENTIFIER
                    && seq_no == sequence
                {
                    if !*neighbor_confirmed {
                        uart.write(b"RF5A_ARP_OK mode=smoltcp-neighbor-cache\r\n");
                        *neighbor_confirmed = true;
                    }
                    let rtt_ms =
                        monotonic_ms().wrapping_sub(started_at).min(u32::MAX as u64) as u32;
                    stats.rx = stats.rx.saturating_add(1);
                    stats.rtt_total_ms = stats.rtt_total_ms.saturating_add(u64::from(rtt_ms));
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
            if received {
                break;
            }
            sleep_poll_interval().await;
        }
        if !received {
            uart.write(b" status=timeout\r\n");
        }
    }

    let dropped = stats.tx.saturating_sub(stats.rx);
    let loss_pct = dropped
        .saturating_mul(100)
        .checked_div(stats.tx)
        .unwrap_or(100);
    uart.write(if stats.rx == 0 {
        b"RF5C_PING_TIMEOUT target="
    } else {
        b"RF5C_PING_OK target="
    });
    write_ipv4(uart, target.octets());
    uart.write(b" tx=0x");
    uart.write(&hex8(stats.tx));
    uart.write(b" rx=0x");
    uart.write(&hex8(stats.rx));
    uart.write(b" drop=0x");
    uart.write(&hex8(dropped));
    uart.write(b" tx_error=0x");
    uart.write(&hex8(stats.tx_errors));
    uart.write(b" loss_pct=0x");
    uart.write(&hex8(loss_pct));
    if stats.rx != 0 {
        uart.write(b" rtt_min_ms=0x");
        uart.write(&hex8(stats.rtt_min_ms));
        uart.write(b" rtt_avg_ms=0x");
        uart.write(&hex8((stats.rtt_total_ms / u64::from(stats.rx)) as u32));
        uart.write(b" rtt_max_ms=0x");
        uart.write(&hex8(stats.rtt_max_ms));
    }
    uart.write(b"\r\n");
    stats
}

fn now() -> Instant {
    Instant::from_millis(monotonic_ms().min(i64::MAX as u64) as i64)
}

async fn sleep_poll_interval() {
    Timer::after(EmbassyDuration::from_millis(POLL_INTERVAL_MS)).await;
}
