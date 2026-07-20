//! Application-owned long-lived IPv4 runner for the A4 Wi-Fi vertical slice.

use core::num::NonZeroU32;

use hisi_hal::uart::Uart;
use hisi_rf_core::WifiDevice;
use smoltcp::iface::{Config, Interface, SocketHandle, SocketSet, SocketStorage};
use smoltcp::phy::{ChecksumCapabilities, Device};
use smoltcp::socket::{dhcpv4, icmp};
use smoltcp::time::{Duration, Instant};
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, Ipv4Address,
};

use super::{hex8, write_ipv4};

const DHCP_TIMEOUT_MS: u64 = 30_000;
const DHCP_SMOKE_MAX_LEASE_SECS: u64 = 20;
const POLL_INTERVAL_MS: u32 = 10;
const PING_TIMEOUT_MS: u64 = 1_000;
const PING_COUNT: u16 = 5;
const ICMP_IDENTIFIER: u16 = 0x5753;
const PUBLIC_TARGET: Ipv4Address = Ipv4Address::new(1, 1, 1, 1);

type Uart0<'a> = Uart<'a, hisi_hal::peripherals::Uart0<'a>>;

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
pub(super) fn run<D: Device>(uart: &Uart0<'_>, mut device: WifiDevice<D>) -> ! {
    let Some(mac) = ws63_rf_rs::netif::hardware_address() else {
        uart.write(b"A4_NET_ERR:no-mac\r\n");
        idle_forever();
    };

    let mut config = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    config.random_seed = 0x5753_3633;
    let mut interface = Interface::new(config, &mut device, now());

    let mut socket_storage = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut socket_storage[..]);
    let mut dhcp_socket = dhcpv4::Socket::new();
    // This smoke-only cap makes renewal observable in one HIL capture without
    // changing the library or production DHCP defaults.
    dhcp_socket.set_max_lease_duration(Some(Duration::from_secs(DHCP_SMOKE_MAX_LEASE_SECS)));
    let dhcp_handle = sockets.add(dhcp_socket);

    let mut icmp_rx_metadata = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_tx_metadata = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_rx_storage = [0_u8; 512];
    let mut icmp_tx_storage = [0_u8; 512];
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
            &mut device,
            &mut sockets,
            dhcp_handle,
            &mut lease,
        );
        sleep_ms(POLL_INTERVAL_MS);
    }

    let Some(active_lease) = lease else {
        uart.write(b"RF5A_DHCP_TIMEOUT rx=0x");
        uart.write(&hex8(ws63_rf_rs::netif::rx_received()));
        uart.write(b" tx=0x");
        uart.write(&hex8(ws63_rf_rs::netif_smoltcp::tx_count()));
        uart.write(b"\r\n");
        keep_polling(
            uart,
            &mut interface,
            &mut device,
            &mut sockets,
            dhcp_handle,
            &mut lease,
            ws63_rf_rs::netif_smoltcp::dhcp_diagnostics(),
        );
    };
    let dhcp_baseline = ws63_rf_rs::netif_smoltcp::dhcp_diagnostics();

    let mut neighbor_confirmed = false;
    let gateway_stats = active_lease
        .router
        .map_or_else(PingStats::default, |gateway| {
            uart.write(b"RF5A_ARP_BEGIN target=");
            write_ipv4(uart, gateway.octets());
            uart.write(b" mode=smoltcp\r\n");
            ping_series(
                uart,
                &mut interface,
                &mut device,
                &mut sockets,
                dhcp_handle,
                icmp_handle,
                &mut lease,
                &mut neighbor_confirmed,
                gateway,
                PING_COUNT,
            )
        });
    let public_stats = ping_series(
        uart,
        &mut interface,
        &mut device,
        &mut sockets,
        dhcp_handle,
        icmp_handle,
        &mut lease,
        &mut neighbor_confirmed,
        PUBLIC_TARGET,
        PING_COUNT,
    );

    let queue = ws63_rf_rs::netif_smoltcp::rx_queue_diagnostics();
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
        &mut device,
        &mut sockets,
        dhcp_handle,
        &mut lease,
        dhcp_baseline,
    )
}

fn keep_polling<D: Device>(
    uart: &Uart0<'_>,
    interface: &mut Interface,
    device: &mut WifiDevice<D>,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    lease: &mut Option<Lease>,
    dhcp_baseline: ws63_rf_rs::netif_smoltcp::DhcpDiagnostics,
) -> ! {
    let mut heartbeat_at = monotonic_ms().saturating_add(10_000);
    let mut renew_reported = false;
    loop {
        poll_network(uart, interface, device, sockets, dhcp_handle, lease);
        let current = monotonic_ms();
        let dhcp = ws63_rf_rs::netif_smoltcp::dhcp_diagnostics();
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
        sleep_ms(POLL_INTERVAL_MS);
    }
}

fn poll_network<D: Device>(
    uart: &Uart0<'_>,
    interface: &mut Interface,
    device: &mut WifiDevice<D>,
    sockets: &mut SocketSet<'_>,
    dhcp_handle: SocketHandle,
    lease: &mut Option<Lease>,
) {
    let timestamp = now();
    let _ = interface.poll(timestamp, device, sockets);
    let event = sockets.get_mut::<dhcpv4::Socket>(dhcp_handle).poll();
    match event {
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
                uart.write(&hex8(next.prefix_len as u32));
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
            interface.update_ip_addrs(|addresses| addresses.clear());
            interface.routes_mut().remove_default_ipv4_route();
            *lease = None;
            uart.write(b"A4_DHCP_DECONFIGURED\r\n");
        }
        None => {}
    }
}

#[allow(clippy::too_many_arguments)]
fn ping_series<D: Device>(
    uart: &Uart0<'_>,
    interface: &mut Interface,
    device: &mut WifiDevice<D>,
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
    uart.write(&hex8(count as u32));
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
        uart.write(&hex8(sequence as u32));
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
            sleep_ms(POLL_INTERVAL_MS);
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

fn monotonic_ms() -> u64 {
    ws63_rf_rs::uapi::monotonic_ms()
}

fn sleep_ms(milliseconds: u32) {
    hisi_rf_rtos_driver::sleep_ms(NonZeroU32::new(milliseconds).expect("non-zero poll interval"))
        .expect("sleep network runner");
}

fn idle_forever() -> ! {
    loop {
        sleep_ms(1_000);
    }
}
