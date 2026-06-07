//! Build script for the semihost_selftest example.
//!
//! Same as the other examples: opt into hisi-riscv-rt's linker scripts. A library
//! dependency's `cargo:rustc-link-arg` does not propagate to a downstream
//! binary, so the binary must request `-Tws63-link.x` itself; hisi-riscv-rt exports
//! its OUT_DIR on the (propagating) link-search path so the `-T` resolves.
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
