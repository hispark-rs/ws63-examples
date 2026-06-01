//! WS63 phase-3 spike: link the vendor Wi-Fi ROM data blob.
//!
//! The north star is connectivity, whose biggest unknown is whether a Rust
//! image can link the closed-source vendor Wi-Fi/BT `.a` blobs at all. This
//! example proves the smallest case end to end, and proves it *completely*: it
//! statically links `ws63-RF/lib/libwifi_rom_data.a` (~3 KB of Wi-Fi ROM
//! configuration data, `rv32imfc`/`ilp32f` — the same ABI as the `ws63`
//! toolchain) with `--whole-archive`, so **all 13** of its config globals land
//! in the image (a config blob must be present in full — the vendor ROM reads
//! every global by address), and it resolves every external symbol it needs:
//!
//! - 13 config globals are DEFINED by the blob; we read and check **all** of
//!   them against the vendor-initialised values.
//! - `__wifi_pkt_ram_begin__` (a linker symbol = the C SDK `.wifi_pkt_ram`
//!   base, 0xA00000) is supplied via `--defsym` in build.rs.
//! - `g_dmac_alg_main` / `g_mac_res_etc` (data the blob points at; real defs
//!   live in the Wi-Fi driver libs) are stubbed below — enough to resolve the
//!   relocations for a link-path proof; the Wi-Fi stack is NOT run here.
//!
//! ## What this de-risks (and what it does NOT)
//!
//! Proven: the ABI matches (`rv32imfc`/`ilp32f`), a vendor static archive links
//! into a Rust image, its `.data` relocations resolve (both data-symbol and
//! linker-symbol kinds), and `--whole-archive` brings the whole config blob in.
//!
//! NOT proven here (left to ROADMAP phase 4): linking the big *code* blobs
//! (`libwifi_driver_dmac.a` ~629 KB with real `.text`, `libbt_host.a` ~1.1 MB)
//! whose many symbols need the real porting layer + HCC IPC; that
//! `g_dmac_alg_main`/`g_mac_res_etc` are satisfied by the *real* driver libs
//! with correct ABI (here they are stubs); and a real reserved `.wifi_pkt_ram`
//! NOLOAD region in `ws63-rt` (here `__wifi_pkt_ram_begin__` is a bare
//! `--defsym`). The Wi-Fi stack does not run — this is a link/relocation proof.

#![no_std]
#![no_main]

use ws63_hal::Peripherals;
use ws63_hal::uart::{Config, Uart};
use ws63_rt::entry;

/// C SDK `.wifi_pkt_ram` region base (linker.lds: 0xA00000, size 0xC000),
/// supplied to the blob as `__wifi_pkt_ram_begin__` via build.rs `--defsym`.
const WIFI_PKT_RAM_BASE: u32 = 0x00A0_0000;

// ── All 13 config globals DEFINED by libwifi_rom_data.a, with their sizes ──
unsafe extern "C" {
    static g_buf_size: u16; // = 40
    static g_skb_size: u16; // = 120
    static g_11b_per_cnt: u32; // = 0x0101_0307
    static g_btcoex_aggr_max_mpdu: u8; // = 0x10
    static g_temp_protect_aggr_max_mpdu: u8; // = 0x10
    static g_mac_pa_switch: u8; // = 0x01
    static g_rf_switch_cfg: u8; // = 0x02
    static g_smooth_phase: u8; // = 0x01
    static g_hal_cfg_custom: [u8; 8]; // = 08 01 01 03 ca 0f 04 01
    static g_ltf_and_gi: [u8; 6]; // = 02 01 01 02 02 01
    /// 56-byte memory-region config; word[2] is a relocation against
    /// `__wifi_pkt_ram_begin__` with addend 4.
    static g_mem_start_addr_cfg: [u32; 14];
    static g_dmac_algorithm_main: u32; // = &g_dmac_alg_main (our stub)
    static g_mac_res: u32; // = &g_mac_res_etc (our stub)
}

// ── Stubs for the data the blob REFERENCES (real defs live in the Wi-Fi
//    driver libs). Dummy storage so each relocation gets a valid address;
//    this is a link-path spike, not a Wi-Fi bring-up. ──
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static g_dmac_alg_main: [u8; 16] = [0; 16];
#[unsafe(no_mangle)]
#[allow(non_upper_case_globals)]
static g_mac_res_etc: [u8; 16] = [0; 16];

/// Render a u32 as 8 lowercase hex digits.
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

/// Render a small count (0..=99) as two decimal digits.
fn dec2(n: u8) -> [u8; 2] {
    [b'0' + (n / 10), b'0' + (n % 10)]
}

/// Volatile read of an extern static (defined in the linked blob object).
#[inline]
fn rd<T: Copy>(p: *const T) -> T {
    unsafe { core::ptr::read_volatile(p) }
}

#[entry]
fn main() -> ! {
    let p = Peripherals::take().unwrap();
    let uart = Uart::new_uart0(p.UART0, Config::default());
    uart.write(0, b"\r\nWS63 Wi-Fi ROM blob link spike\r\n");

    // Read every config global (volatile: defined in another object). Reading
    // them all also keeps every section live under --gc-sections.
    let buf = rd(&raw const g_buf_size);
    let skb = rd(&raw const g_skb_size);
    let per_cnt = rd(&raw const g_11b_per_cnt);
    let btcoex = rd(&raw const g_btcoex_aggr_max_mpdu);
    let tprot = rd(&raw const g_temp_protect_aggr_max_mpdu);
    let pa_sw = rd(&raw const g_mac_pa_switch);
    let rf_sw = rd(&raw const g_rf_switch_cfg);
    let smooth = rd(&raw const g_smooth_phase);
    let hal_cfg = rd(&raw const g_hal_cfg_custom);
    let ltf_gi = rd(&raw const g_ltf_and_gi);
    let region1 = rd((&raw const g_mem_start_addr_cfg)
        .cast::<u32>()
        .wrapping_add(2));
    let dmac_ptr = rd(&raw const g_dmac_algorithm_main);
    let mac_res_ptr = rd(&raw const g_mac_res);

    let dmac_stub = &raw const g_dmac_alg_main as u32;
    let mac_stub = &raw const g_mac_res_etc as u32;

    // Headline values (the rest fold into the 13/13 check below).
    let report = [
        (&b"g_buf_size           = 0x"[..], buf as u32),
        (&b"g_skb_size           = 0x"[..], skb as u32),
        (&b"g_mem_start_addr[2]  = 0x"[..], region1),
        (&b"g_dmac_algorithm_main= 0x"[..], dmac_ptr),
        (&b"&g_dmac_alg_main stub= 0x"[..], dmac_stub),
    ];
    for (label, val) in report {
        uart.write(0, label);
        uart.write(0, &hex8(val));
        uart.write(0, b"\r\n");
    }

    // Verify ALL 13 config globals carry their vendor-initialised values and
    // that both relocation kinds resolved.
    let checks = [
        buf == 40,              // .data linked
        skb == 120,             // .data linked
        per_cnt == 0x0101_0307, // 07 03 01 01 (LE)
        btcoex == 0x10,         //
        tprot == 0x10,          //
        pa_sw == 0x01,          //
        rf_sw == 0x02,          //
        smooth == 0x01,         //
        hal_cfg == [0x08, 0x01, 0x01, 0x03, 0xca, 0x0f, 0x04, 0x01],
        ltf_gi == [0x02, 0x01, 0x01, 0x02, 0x02, 0x01],
        region1 == WIFI_PKT_RAM_BASE + 4, // __wifi_pkt_ram_begin__ reloc
        dmac_ptr == dmac_stub,            // data-symbol reloc -> stub
        mac_res_ptr == mac_stub,          // data-symbol reloc -> stub
    ];
    let passed = checks.iter().filter(|&&c| c).count();

    uart.write(0, b"config globals verified: ");
    uart.write(0, &dec2(passed as u8));
    uart.write(0, b"/");
    uart.write(0, &dec2(checks.len() as u8));
    uart.write(0, b"\r\n");

    uart.write(
        0,
        if passed == checks.len() {
            b"BLOB LINK SPIKE: PASS\r\n"
        } else {
            b"BLOB LINK SPIKE: FAIL\r\n"
        },
    );

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
