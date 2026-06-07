//! Build script for the gpio_irq example (links via hisi-riscv-rt's exported scripts).
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
