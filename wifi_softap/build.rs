fn main() {
    // The final firmware owns the runtime entry script. RF archive and ROM
    // roots remain transitive through the facade-selected chip backend.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");
}
