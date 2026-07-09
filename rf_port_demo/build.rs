//! Build script for the RF porting-layer demo.
//!
//! Links the ws63-rf-rs porting layer (a normal crate dependency) plus the
//! vendor ROM-data blob. The Wi-Fi packet-RAM linker symbols the blob references
//! are now supplied by hisi-riscv-rt's WS63 `.wifi_pkt_ram` NOLOAD section. The
//! blob's external data symbols (`g_dmac_alg_main`,
//! `g_mac_res_etc`) resolve to ws63-rf-rs's `globals` module.
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let lib_dir = PathBuf::from(&manifest).join("../../../chips/ws63/rf/ws63-RF/lib");
    let lib_dir = lib_dir.canonicalize().unwrap_or(lib_dir);
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // Pull in the whole config blob (it is consumed by address, not by symbol
    // reference) — see ws63-examples/wifi_blob_link (phase 3).
    println!("cargo:rustc-link-lib=static:+whole-archive=wifi_rom_data");

    println!("cargo:rerun-if-changed=build.rs");
}
