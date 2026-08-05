fn main() {
    // The final firmware owns the runtime entry script. RF archive and ROM
    // roots remain transitive through the facade-selected chip backend.
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    if std::env::var_os("CARGO_FEATURE_DATA_PATH_DIAGNOSTICS").is_some() {
        // Final-link-only diagnostic wrapping cannot propagate from a Cargo
        // dependency. Keep this opt-in HIL fixture aligned with the station
        // connectivity fixture; ordinary consumer builds do not enable it.
        println!("cargo:rustc-link-arg=--wrap=hmac_bridge_vap_xmit_etc");
        println!("cargo:rustc-link-arg=--wrap=dmac_tx_complete_event_handler");
        println!("cargo:rustc-link-arg=--wrap=dmac_rx_prepare_data_patch");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_data_event_adapt");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_process_data_msg");
        println!("cargo:rustc-link-arg=--wrap=hmac_rx_data");
    }
}
