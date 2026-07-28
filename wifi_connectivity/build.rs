fn main() {
    // The final binary owns its runtime entry script. RF archives, ROM patches,
    // NVS symbols, and archive ordering remain behind the hisi-rf facade.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");
}
