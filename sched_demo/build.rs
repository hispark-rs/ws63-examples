//! Build script for the sched_demo example (links via ws63-rt's linker scripts).
//! No vendor blob is linked — this exercises only the ws63-rf-rs scheduler.
fn main() {
    println!("cargo:rustc-link-arg=-Tws63-link.x");
    println!("cargo:rerun-if-changed=build.rs");
}
