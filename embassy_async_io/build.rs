fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
