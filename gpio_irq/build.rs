//! Build script for the gpio_irq example (links via hisi-riscv-rt's exported scripts).
fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
