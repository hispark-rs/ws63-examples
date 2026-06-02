//! Build script for the custom_memory example.
//!
//! This example owns its `memory.x` (ws63-rt's bundled one is disabled via
//! `default-features = false`). Copy ours into OUT_DIR and put that dir on the
//! linker search path, so `ws63-link.x`'s `INCLUDE memory.x` resolves to THIS
//! file. Because ws63-rt is not also emitting a memory.x, there is exactly one
//! on the search path — deterministic, no link-order ambiguity.
use std::env;
use std::fs::File;
use std::io::Write;
use std::path::PathBuf;

fn main() {
    let out = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    File::create(out.join("memory.x"))
        .unwrap()
        .write_all(include_bytes!("memory.x"))
        .unwrap();
    println!("cargo:rustc-link-search={}", out.display());
    println!("cargo:rerun-if-changed=memory.x");

    // ws63-rt still supplies layout.ld / device.x / riscv-rt-symbols.x and the
    // ws63-link.x entry script that INCLUDEs all four (incl. our memory.x).
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
