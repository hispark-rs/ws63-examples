//! Build script for the net_ping example.
//!
//! Same as blinky: opt into ws63-rt's linker script (a library dependency's
//! `cargo:rustc-link-arg` does not propagate to a downstream binary, so the
//! binary must request `-Tws63-link.x` itself; ws63-rt exports the search path).
//! No vendor blob is linked — this example is pure smoltcp over the QEMU MAC.
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
