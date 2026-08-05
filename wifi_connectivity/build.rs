fn main() {
    // The final binary owns its runtime entry script. RF archives, ROM patches,
    // NVS symbols, and archive ordering remain behind the hisi-rf facade.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    // A5B migration diagnostic: dependency link arguments do not propagate to
    // the final binary, so this fixture explicitly activates the secret-free
    // synchronous HMAC-message timing wrapper supplied by the facade-selected backend.
    println!("cargo:rustc-link-arg=--wrap=frw_sync_host_post_msg");

    if std::env::var_os("CARGO_FEATURE_DATA_PATH_DIAGNOSTICS").is_some() {
        // Low-disturbance packet-path instrumentation is intentionally owned
        // by this opt-in HIL fixture. Cargo does not propagate a dependency's
        // `rustc-link-arg` to the final binary.
        println!("cargo:rustc-link-arg=--wrap=dmac_tx_complete_event_handler");
        println!("cargo:rustc-link-arg=--wrap=dmac_rx_prepare_data_patch");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_data_event_adapt");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_process_data_msg");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_data");
        println!("cargo:rustc-link-arg=--wrap=hmac_tx_lan_to_wlan_no_tcp_opt_etc");
        println!("cargo:rustc-link-arg=--wrap=hmac_tx_process_data");
        println!("cargo:rustc-link-arg=--wrap=hmac_tx_data_send");
        println!("cargo:rustc-link-arg=--wrap=frw_hmac_send_data");
        println!("cargo:rustc-link-arg=--wrap=dmac_tx_process_data_event");
    }
}
