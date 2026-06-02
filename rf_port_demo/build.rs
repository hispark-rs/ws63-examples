//! Build script for the rf_port_demo (phase-4) example.
//!
//! Links the ws63-rf-rs porting layer (a normal crate dependency) plus the
//! vendor ROM-data blob, and supplies the Wi-Fi packet-RAM linker symbols the
//! blob references. The blob's external data symbols (`g_dmac_alg_main`,
//! `g_mac_res_etc`) resolve to ws63-rf-rs's `globals` module.
use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");

    let manifest = std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");
    let lib_dir = PathBuf::from(&manifest).join("../../ws63-rf-rs/ws63-RF/lib");
    let lib_dir = lib_dir.canonicalize().unwrap_or(lib_dir);
    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    // Pull in the whole config blob (it is consumed by address, not by symbol
    // reference) — see ws63-examples/wifi_blob_link (phase 3).
    println!("cargo:rustc-link-lib=static:+whole-archive=wifi_rom_data");

    // Wi-Fi packet-RAM region (C SDK .wifi_pkt_ram base/size). Scaffold: an
    // absolute --defsym; a reserved NOLOAD region is ROADMAP phase 4.
    println!("cargo:rustc-link-arg=--defsym=__wifi_pkt_ram_begin__=0x00A00000");
    println!("cargo:rustc-link-arg=--defsym=__wifi_pkt_ram_end__=0x00A0C000");

    println!("cargo:rerun-if-changed=build.rs");
}
