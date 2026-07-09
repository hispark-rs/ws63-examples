//! Build script for the RF init smoke.
//!
//! This links the complete Wi-Fi init closure against ws63-rf-rs, the WS63 ROM
//! symbol table, and hisi-riscv-rt's memory layout. The example is intentionally
//! small: build/link success proves the firmware image can carry the init
//! closure; UART output then separates early boot from the vendor init result.

use std::path::PathBuf;

fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let rf = manifest.join("../../../chips/ws63/rf/ws63-RF");
    let rf = rf.canonicalize().unwrap_or(rf);
    let lib_dir = rf.join("lib");
    let rom = rf.join("rom/ws63_acore_rom.lds");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    if std::env::var_os("CARGO_FEATURE_FULL_INIT").is_some() {
        println!("cargo:rustc-link-arg=-T{}", rom.display());
        for lib in [
            "wifi_driver_hmac",
            "wifi_driver_dmac",
            "wifi_driver_tcm",
            "bg_common",
            "wifi_alg_anti_interference",
            "wifi_alg_cca_opt",
            "wifi_alg_edca_opt",
            "wifi_alg_temp_protect",
            "wifi_alg_txbf",
            "wifi_rom_data",
        ] {
            println!("cargo:rustc-link-lib=static={lib}");
        }
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", rom.display());
    println!("cargo:rerun-if-changed={}", lib_dir.display());
}
