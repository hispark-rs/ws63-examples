//! Build script for the wifi_blob_link phase-3 spike.
//!
//! Besides the usual hisi-riscv-rt linker-script opt-in (`-Thisi-riscv-link.x`), this links
//! the vendor Wi-Fi ROM data archive. The packet-RAM linker symbols it references
//! are supplied by hisi-riscv-rt's WS63 `.wifi_pkt_ram` NOLOAD section:
//!
//! - `libwifi_rom_data.a` lives in the `ws63-RF` submodule (`../../ws63-rf-rs/ws63-RF/lib`).
//!   It is `rv32imfc` / `ilp32f` (single-float), matching the `ws63` toolchain's
//!   `riscv32imfc-unknown-none-elf` target, so the ABI lines up.
//! - `__wifi_pkt_ram_begin__` is the base of the C SDK `.wifi_pkt_ram` region
//!   (`linker.lds`: `0xA00000`, size `0xC000`). The blob's `g_mem_start_addr_cfg`
//!   stores `base + <offset>` words against it.
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

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

    println!("cargo:rerun-if-changed=build.rs");
    println!(
        "cargo:rerun-if-changed={}/libwifi_rom_data.a",
        lib_dir.display()
    );
}
