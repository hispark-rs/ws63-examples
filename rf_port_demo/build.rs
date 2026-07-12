//! Build script for the RF porting-layer demo.
//!
//! This deliberately does not link a vendor archive. `wifi_blob_link` owns the
//! minimal raw-blob link check; `wifi_init_smoke` owns the complete runtime.

fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    println!("cargo:rerun-if-changed=build.rs");
}
