//! Build script for the async_delay example — opt into ws63-rt's linker scripts.
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
