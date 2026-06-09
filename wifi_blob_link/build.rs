//! Build script for the wifi_blob_link phase-3 spike.
//!
//! Besides the usual hisi-riscv-rt linker-script opt-in (`-Tws63-link.x`), this links
//! the vendor Wi-Fi ROM data archive and supplies the one linker symbol it
//! references:
//!
//! - `libwifi_rom_data.a` lives in the `ws63-RF` submodule (`../../ws63-rf-rs/ws63-RF/lib`).
//!   It is `rv32imfc` / `ilp32f` (single-float), matching the `ws63` toolchain's
//!   `riscv32imfc-unknown-none-elf` target, so the ABI lines up.
//! - `__wifi_pkt_ram_begin__` is the base of the C SDK `.wifi_pkt_ram` region
//!   (`linker.lds`: `0xA00000`, size `0xC000`). The blob's `g_mem_start_addr_cfg`
//!   stores `base + <offset>` words against it; we provide it via `--defsym`.
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let lib_dir = PathBuf::from(&manifest).join("../../../chips/ws63/rf/ws63-RF/lib");
    let lib_dir = lib_dir.canonicalize().unwrap_or(lib_dir);
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // `+whole-archive`: pull in EVERY object/section of the config archive, not
    // just the symbols our Rust code happens to reference. A Wi-Fi ROM *config*
    // blob must be present in full (the vendor ROM/driver reads all of its
    // globals by address), and it must not depend on a downstream reference to
    // be linked at all. Matches how the C SDK unconditionally includes it.
    println!("cargo:rustc-link-lib=static:+whole-archive=wifi_rom_data");

    // The C SDK .wifi_pkt_ram region; the blob references its bounds as linker
    // symbols (base 0x00A0_0000, size 0xC000 -> end 0x00A0_C000, per the C SDK
    // linker.lds). These two are the ENTIRE residual of a Wi-Fi-init link once
    // ws63-rf-rs + the WS63 ROM symbol table resolve everything else — see
    // ws63-rf-rs/tools/mac-link-residual.sh.
    println!("cargo:rustc-link-arg=--defsym=__wifi_pkt_ram_begin__=0x00A00000");
    println!("cargo:rustc-link-arg=--defsym=__wifi_pkt_ram_end__=0x00A0C000");

    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}/libwifi_rom_data.a",
        lib_dir.display()
    );
}
