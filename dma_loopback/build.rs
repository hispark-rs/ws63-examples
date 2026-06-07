//! Build script for the dma_loopback example.
//!
//! The WS63 linker scripts live in hisi-riscv-rt. A library dependency's
//! `cargo:rustc-link-arg` does NOT propagate to a downstream binary, so the
//! binary must opt in itself. hisi-riscv-rt exposes its OUT_DIR (containing
//! `ws63-link.x` + the four scripts it INCLUDEs) via `cargo:rustc-link-search`,
//! which DOES propagate here, so this single `-T` resolves correctly.
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
