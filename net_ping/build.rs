//! Build script for the net_ping example.
//!
//! Same as blinky: opt into hisi-riscv-rt's linker script (a library dependency's
//! `cargo:rustc-link-arg` does not propagate to a downstream binary, so the
//! binary must request `-Thisi-riscv-link.x` itself; hisi-riscv-rt exports the search path).
//! No vendor blob is linked — this example is pure smoltcp over the QEMU MAC.
fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
