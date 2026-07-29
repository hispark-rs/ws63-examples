fn main() {
    // The final binary owns its runtime entry script. RF archives, ROM patches,
    // NVS symbols, and archive ordering remain behind the hisi-rf facade.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    // A5B migration diagnostic: dependency link arguments do not propagate to
    // the final binary, so this fixture explicitly activates the secret-free
    // synchronous HMAC-message timing wrapper supplied by hisi-rf-ws63.
    println!("cargo:rustc-link-arg=--wrap=frw_sync_host_post_msg");
}
