//! WS63 connectivity demo (ROADMAP phase 5, M3) — a real TCP/IP round-trip over
//! the `ws63-qemu` **synthetic Wi-Fi/Ethernet MAC** (`ws63-netmac` @ 0x4421_0000)
//! and the SLIRP user-mode NAT. NO radio: this validates the *software* path —
//! `ws63-rf-rs`'s netif→smoltcp bridge driving an actual MMIO MAC + SLIRP — not
//! the closed RF/PHY blob (which is hardware-in-the-loop only).
//!
//! Data path:
//! - **TX**: smoltcp emits a frame → bridge `TxToken` → our `set_tx_sink` writes
//!   it into the MAC's `TX_BUF` + `TX_LEN` and pulses `TX_GO` → QEMU hands it to
//!   the netdev (`qemu_send_packet` → SLIRP).
//! - **RX**: SLIRP delivers a frame → QEMU MAC raises **IRQ 45 (WLMAC_INT)** →
//!   our trap handler reads `RX_BUF`/`RX_LEN`, calls the bridge's `rx_push`, and
//!   `RX_ACK`s the MAC → smoltcp drains it on the next `Interface::poll`.
//!
//! With a static IP `10.0.2.15/24` and default gateway `10.0.2.2` (the SLIRP
//! gateway), it ARP-resolves the gateway, ICMP-pings it (SLIRP answers locally —
//! no external network needed), and sends one UDP datagram. Prints over UART0;
//! `NET PING: PASS` once an ICMP echo reply arrives.
//!
//! Run on ws63-qemu with a user netdev (the default `-nic user` provides one):
//!   qemu-system-riscv32 -M ws63 -nographic -nic user -kernel net_ping
//! Build:
//!   cargo build -p net_ping --release

#![no_std]
#![no_main]

use core::ptr::{read_volatile, write_volatile};

use hisi_riscv_hal::Peripherals;
use hisi_riscv_hal::interrupt::{self, Interrupt};
use hisi_riscv_hal::uart::{Config as UartConfig, Uart};
use hisi_riscv_rt::entry;
use smoltcp::iface::{Config, Interface, SocketSet, SocketStorage};
use smoltcp::phy::Device;
use smoltcp::socket::{icmp, udp};
use smoltcp::time::Instant;
use smoltcp::wire::{
    EthernetAddress, HardwareAddress, Icmpv4Packet, Icmpv4Repr, IpAddress, IpCidr, IpEndpoint,
    Ipv4Address,
};
use ws63_rf_rs::netif_smoltcp::{Ws63Device, rx_push};

// ── ws63-netmac register map (must match ws63-qemu hw/riscv/ws63.c) ──────────
const NETMAC: usize = 0x4421_0000;
const NM_CTRL: usize = NETMAC; // +0x000: bit0 enable, bit1 rx_irq_en
const NM_INT_STATUS: usize = NETMAC + 0x004; // bit0 rx_ready
const NM_TX_LEN: usize = NETMAC + 0x00C;
const NM_TX_GO: usize = NETMAC + 0x010;
const NM_RX_LEN: usize = NETMAC + 0x014;
const NM_RX_ACK: usize = NETMAC + 0x018;
const NM_MAC_LO: usize = NETMAC + 0x020;
const NM_MAC_HI: usize = NETMAC + 0x024;
const NM_TX_BUF: usize = NETMAC + 0x1000;
const NM_RX_BUF: usize = NETMAC + 0x2000;

const NM_EN: u32 = 0x1;
const NM_RX_IRQ_EN: u32 = 0x2;
const NM_INT_RX: u32 = 0x1;
const NM_BUF_MAX: usize = 2048;

const WLMAC_IRQ: u32 = Interrupt::WLMAC_INT as u32; // 45

// 24 MHz free-running TCXO counter — a monotonic ms time base for smoltcp.
const TCXO_LO: usize = 0x4400_04C4;
const TCXO_HI: usize = 0x4400_04C8;

// ── TX sink: smoltcp frame → MAC TX registers (plain fn, no captures) ────────
fn mac_tx(frame: &[u8]) {
    let n = frame.len().min(NM_BUF_MAX);
    unsafe {
        let mut i = 0;
        while i < n {
            let mut w = 0u32;
            let mut b = 0;
            while b < 4 && i + b < n {
                w |= (frame[i + b] as u32) << (8 * b);
                b += 1;
            }
            write_volatile((NM_TX_BUF + i) as *mut u32, w);
            i += 4;
        }
        write_volatile(NM_TX_LEN as *mut u32, n as u32);
        write_volatile(NM_TX_GO as *mut u32, 1);
    }
}

fn now_ms() -> u64 {
    // Read low first (the QEMU model advances the counter on the low read), then
    // high — they observe the same advanced value.
    unsafe {
        let lo = read_volatile(TCXO_LO as *const u32) as u64;
        let hi = read_volatile(TCXO_HI as *const u32) as u64;
        ((hi << 32) | lo) / 24_000
    }
}

// ── IRQ 45 (WLMAC_INT) trap: drain the MAC RX buffer into the bridge ─────────
// Own direct-mode mtvec (same approach as the gpio_irq example) so we don't
// collide with hisi-riscv-rt's weak cross-crate trap hooks. The handler reads mcause,
// and on WLMAC_INT copies the frame out of RX_BUF, hands it to `rx_push`, and
// RX_ACKs the MAC (which drops the level-triggered IRQ line) before clearing the
// local pending bit — order matters, or the still-high line would re-pend.
core::arch::global_asm!(
    ".section .text.nmirq, \"ax\"",
    ".balign 4",
    ".global nmac_trap",
    "nmac_trap:",
    "    addi sp, sp, -64",
    "    sw ra,0(sp)",
    "    sw t0,4(sp)",
    "    sw t1,8(sp)",
    "    sw t2,12(sp)",
    "    sw t3,16(sp)",
    "    sw t4,20(sp)",
    "    sw t5,24(sp)",
    "    sw t6,28(sp)",
    "    sw a0,32(sp)",
    "    sw a1,36(sp)",
    "    sw a2,40(sp)",
    "    sw a3,44(sp)",
    "    sw a4,48(sp)",
    "    sw a5,52(sp)",
    "    sw a6,56(sp)",
    "    sw a7,60(sp)",
    "    call nmac_handle",
    "    lw ra,0(sp)",
    "    lw t0,4(sp)",
    "    lw t1,8(sp)",
    "    lw t2,12(sp)",
    "    lw t3,16(sp)",
    "    lw t4,20(sp)",
    "    lw t5,24(sp)",
    "    lw t6,28(sp)",
    "    lw a0,32(sp)",
    "    lw a1,36(sp)",
    "    lw a2,40(sp)",
    "    lw a3,44(sp)",
    "    lw a4,48(sp)",
    "    lw a5,52(sp)",
    "    lw a6,56(sp)",
    "    lw a7,60(sp)",
    "    addi sp, sp, 64",
    "    mret",
);

unsafe extern "C" {
    fn nmac_trap();
}

static mut IRQ_HITS: u32 = 0;

/// Move one pending RX frame out of the MAC into the smoltcp bridge; returns true
/// if a frame was drained. Called from the WLMAC interrupt handler. NOTE: keep
/// the MTU-sized stack buffer in this *called* function, NOT inlined into the
/// trap handler — the handler's hand-rolled asm frame can't host a 2 KB local
/// without overflowing, which silently breaks RX delivery.
fn drain_rx() -> bool {
    unsafe {
        if read_volatile(NM_INT_STATUS as *const u32) & NM_INT_RX == 0 {
            return false;
        }
        let len = (read_volatile(NM_RX_LEN as *const u32) as usize).min(NM_BUF_MAX);
        let mut frame = [0u8; NM_BUF_MAX];
        let mut i = 0;
        while i < len {
            let w = read_volatile((NM_RX_BUF + i) as *const u32);
            let mut b = 0;
            while b < 4 && i + b < len {
                frame[i + b] = (w >> (8 * b)) as u8;
                b += 1;
            }
            i += 4;
        }
        rx_push(&frame[..len]);
        write_volatile(NM_RX_ACK as *mut u32, 1); // release buffer + drop IRQ line
        true
    }
}

#[unsafe(no_mangle)]
extern "C" fn nmac_handle() {
    let mcause: u32;
    unsafe { core::arch::asm!("csrr {0}, mcause", out(reg) mcause) };
    if (mcause & 0x8000_0000) == 0 || (mcause & 0xFFF) != WLMAC_IRQ {
        return;
    }
    if drain_rx() {
        unsafe { IRQ_HITS = IRQ_HITS.wrapping_add(1) };
    }
    interrupt::clear_pending(Interrupt::WLMAC_INT); // LOCIPCLR
}

// ── tiny UART formatting helpers ─────────────────────────────────────────────
type Uart0<'a> = Uart<'a, hisi_riscv_hal::peripherals::Uart0<'a>>;

fn put_u32(uart: &Uart0<'_>, mut n: u32) {
    let mut buf = [0u8; 10];
    let s: &[u8] = if n == 0 {
        buf[0] = b'0';
        &buf[..1]
    } else {
        let mut i = buf.len();
        while n > 0 {
            i -= 1;
            buf[i] = b'0' + (n % 10) as u8;
            n /= 10;
        }
        &buf[i..]
    };
    uart.write(0, s);
}

fn put_mac(uart: &Uart0<'_>, mac: &[u8; 6]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 17];
    let mut k = 0;
    for (j, b) in mac.iter().enumerate() {
        if j > 0 {
            out[k] = b':';
            k += 1;
        }
        out[k] = HEX[(b >> 4) as usize];
        out[k + 1] = HEX[(b & 0xF) as usize];
        k += 2;
    }
    uart.write(0, &out[..k]);
}

const PING_PAYLOAD: [u8; 16] = *b"ws63-rs ping....";

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, UartConfig::default());
    uart.write(
        0,
        b"\r\nWS63 net_ping: smoltcp over ws63-netmac + SLIRP (no RF)\r\n",
    );

    // 1. Bring up the MAC: read its hardware MAC, enable RX + RX interrupt.
    let (lo, hi) = unsafe {
        (
            read_volatile(NM_MAC_LO as *const u32),
            read_volatile(NM_MAC_HI as *const u32),
        )
    };
    let mac = [
        lo as u8,
        (lo >> 8) as u8,
        (lo >> 16) as u8,
        (lo >> 24) as u8,
        hi as u8,
        (hi >> 8) as u8,
    ];
    uart.write(0, b"mac             = ");
    put_mac(&uart, &mac);
    uart.write(0, b"\r\nip              = 10.0.2.15/24 gw 10.0.2.2\r\n");
    unsafe { write_volatile(NM_CTRL as *mut u32, NM_EN | NM_RX_IRQ_EN) };

    // 2. Install the TX sink + the WLMAC RX interrupt vector.
    ws63_rf_rs::netif_smoltcp::set_tx_sink(mac_tx);
    unsafe {
        core::arch::asm!("csrw mtvec, {0}", in(reg) nmac_trap as *const () as usize); // direct mode
        interrupt::init();
        interrupt::enable(Interrupt::WLMAC_INT);
        interrupt::enable_global();
    }

    // 3. smoltcp interface: static IP + default route via the SLIRP gateway.
    let mut dev = Ws63Device;
    let checksum = dev.capabilities().checksum;
    let cfg = Config::new(HardwareAddress::Ethernet(EthernetAddress(mac)));
    let mut iface = Interface::new(cfg, &mut dev, Instant::from_millis(now_ms() as i64));
    iface.update_ip_addrs(|a| {
        let _ = a.push(IpCidr::new(
            IpAddress::Ipv4(Ipv4Address::new(10, 0, 2, 15)),
            24,
        ));
    });
    iface
        .routes_mut()
        .add_default_ipv4_route(Ipv4Address::new(10, 0, 2, 2))
        .unwrap();
    let gw = IpAddress::Ipv4(Ipv4Address::new(10, 0, 2, 2));

    // 4. Sockets: one ICMP (echo) + one UDP (TX demo).
    let mut icmp_rx_meta = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_rx_data = [0u8; 512];
    let mut icmp_tx_meta = [icmp::PacketMetadata::EMPTY; 4];
    let mut icmp_tx_data = [0u8; 512];
    let icmp_sock = icmp::Socket::new(
        icmp::PacketBuffer::new(&mut icmp_rx_meta[..], &mut icmp_rx_data[..]),
        icmp::PacketBuffer::new(&mut icmp_tx_meta[..], &mut icmp_tx_data[..]),
    );
    let mut udp_rx_meta = [udp::PacketMetadata::EMPTY; 2];
    let mut udp_rx_data = [0u8; 256];
    let mut udp_tx_meta = [udp::PacketMetadata::EMPTY; 2];
    let mut udp_tx_data = [0u8; 256];
    let udp_sock = udp::Socket::new(
        udp::PacketBuffer::new(&mut udp_rx_meta[..], &mut udp_rx_data[..]),
        udp::PacketBuffer::new(&mut udp_tx_meta[..], &mut udp_tx_data[..]),
    );

    let mut sock_store = [SocketStorage::EMPTY; 2];
    let mut sockets = SocketSet::new(&mut sock_store[..]);
    let icmp_h = sockets.add(icmp_sock);
    let udp_h = sockets.add(udp_sock);

    const IDENT: u16 = 0x1234;
    let mut seq: u16 = 0;
    let mut received: u32 = 0;
    let mut udp_sent = false;
    let mut next_send: u64 = 0;
    let start = now_ms();

    // 5. Poll loop: ARP-resolve, ping, send a UDP datagram; stop on first reply.
    loop {
        let now = now_ms();
        iface.poll(Instant::from_millis(now as i64), &mut dev, &mut sockets);

        {
            let s = sockets.get_mut::<icmp::Socket>(icmp_h);
            if !s.is_open() {
                s.bind(icmp::Endpoint::Ident(IDENT)).unwrap();
            }
            if s.can_send() && seq < 20 && now >= next_send {
                let repr = Icmpv4Repr::EchoRequest {
                    ident: IDENT,
                    seq_no: seq,
                    data: &PING_PAYLOAD,
                };
                if let Ok(buf) = s.send(repr.buffer_len(), gw) {
                    let mut pkt = Icmpv4Packet::new_unchecked(buf);
                    repr.emit(&mut pkt, &checksum);
                    uart.write(0, b"ping  seq=");
                    put_u32(&uart, seq as u32);
                    uart.write(0, b" -> 10.0.2.2\r\n");
                }
                seq += 1;
                next_send = now + 100;
            }
            if s.can_recv()
                && let Ok((payload, _from)) = s.recv()
                && let Ok(pkt) = Icmpv4Packet::new_checked(payload)
                && let Ok(Icmpv4Repr::EchoReply { seq_no, .. }) = Icmpv4Repr::parse(&pkt, &checksum)
            {
                received += 1;
                uart.write(0, b"reply seq=");
                put_u32(&uart, seq_no as u32);
                uart.write(0, b" <- 10.0.2.2\r\n");
            }
        }

        // One-shot UDP TX (SLIRP does not echo UDP; this proves the TX path).
        if !udp_sent {
            let u = sockets.get_mut::<udp::Socket>(udp_h);
            if !u.is_open() {
                u.bind(6800u16).unwrap();
            }
            if u.can_send()
                && u.send_slice(b"ws63-rs net_ping\n", IpEndpoint::new(gw, 9))
                    .is_ok()
            {
                udp_sent = true;
                uart.write(0, b"udp   tx -> 10.0.2.2:9 (16 bytes)\r\n");
            }
        }

        if received >= 1 {
            uart.write(0, b"rx irq hits     = ");
            put_u32(&uart, unsafe { read_volatile(&raw const IRQ_HITS) });
            uart.write(0, b"\r\nNET PING: PASS\r\n");
            break;
        }
        if now.saturating_sub(start) > 5000 {
            uart.write(0, b"rx irq hits     = ");
            put_u32(&uart, unsafe { read_volatile(&raw const IRQ_HITS) });
            uart.write(0, b"\r\nNET PING: FAIL (no echo reply)\r\n");
            break;
        }
        for _ in 0..20_000 {
            core::hint::spin_loop();
        }
    }

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
