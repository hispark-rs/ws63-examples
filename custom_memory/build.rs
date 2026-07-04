//! Build script for the custom_memory example.
//!
//! This example owns its `memory.x` (hisi-riscv-rt's bundled one is disabled via
//! `default-features = false`). Copy ours into OUT_DIR and put that dir on the
//! linker search path, so `hisi-riscv-link.x`'s `INCLUDE memory.x` resolves to THIS
//! file. Because hisi-riscv-rt is not also emitting a memory.x, there is exactly one
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

    // hisi-riscv-rt still supplies layout.ld / riscv-rt-symbols.x and the
    // hisi-riscv-link.x entry script; ws63-pac/rt supplies device.x, and this
    // example supplies memory.x.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
