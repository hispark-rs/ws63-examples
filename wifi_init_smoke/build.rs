//! Build script for the RF init smoke.
//!
//! This links the complete Wi-Fi init closure against ws63-rf-rs, the WS63 ROM
//! symbol table, and hisi-riscv-rt's memory layout. The example is intentionally
//! small: build/link success proves the firmware image can carry the init
//! closure; UART output then separates early boot from the vendor init result.

use std::{
    fs,
    path::{Path, PathBuf},
};

fn write_rom_fallbacks(source: &Path, output: &Path) {
    let source_text = fs::read_to_string(source).expect("read WS63 ROM symbol table");
    let mut generated = String::with_capacity(source_text.len() + 32 * 1024);

    generated.push_str("/* Generated from ws63_acore_rom.lds. */\n");
    generated.push_str("/* Application definitions override these mask-ROM fallbacks. */\n");

    for line in source_text.lines() {
        let trimmed = line.trim();
        if let Some((name, value)) = trimmed
            .strip_suffix(';')
            .and_then(|line| line.split_once(" = "))
        {
            assert!(
                !name.is_empty()
                    && name.bytes().all(|byte| {
                        byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'.' | b'$')
                    }),
                "invalid ROM symbol name: {name:?}"
            );
            assert!(
                value.starts_with("0x") && value[2..].bytes().all(|byte| byte.is_ascii_hexdigit()),
                "invalid ROM symbol value for {name}: {value:?}"
            );
            generated.push_str("PROVIDE(");
            generated.push_str(name);
            generated.push_str(" = ");
            generated.push_str(value);
            generated.push_str(");\n");
        }
    }

    fs::write(output, generated).expect("write generated WS63 ROM fallbacks");
}

fn write_rom_callback_fallbacks(source: &Path, output: &Path) {
    let source_text = fs::read_to_string(source).expect("read WS63 ROM callback list");
    let mut generated = String::with_capacity(source_text.len() * 4);

    generated.push_str("/* Generated from the ordered WS63 mask-ROM callback ABI. */\n");
    generated.push_str("/* A strong application symbol wins; an absent callback traps. */\n");

    for name in source_text.lines().map(str::trim) {
        if name.is_empty() || name.starts_with('#') {
            continue;
        }
        assert!(
            name.bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_'),
            "invalid ROM callback name: {name:?}"
        );
        let target = match name {
            "__ashldi3" => "__ws63_ashldi3",
            "__udivdi3" => "__ws63_udivdi3",
            "__umoddi3" => "__ws63_umoddi3",
            "memcmp" => "__ws63_rom_memcmp",
            "memcpy" => "__ws63_rom_memcpy",
            "memmove" => "__ws63_rom_memmove",
            "memset" => "__ws63_rom_memset",
            "strlen" => "__ws63_rom_strlen",
            name if matches!(
                name,
                "log_event_print0"
                    | "log_event_print1"
                    | "log_event_print2"
                    | "log_event_print3"
                    | "log_event_print4"
                    | "log_event_wifi_print0"
                    | "log_event_wifi_print1"
                    | "log_event_wifi_print2"
                    | "log_event_wifi_print3"
                    | "log_event_wifi_print4"
                    | "osal_irq_clear"
                    | "osal_irq_disable"
                    | "osal_irq_enable"
                    | "osal_irq_free"
                    | "osal_irq_lock"
                    | "osal_irq_request"
                    | "osal_irq_restore"
                    | "osal_irq_set_priority"
                    | "osal_kfree"
                    | "osal_kmalloc"
                    | "osal_kthread_lock"
                    | "osal_kthread_unlock"
                    | "osal_timer_destroy"
                    | "osal_timer_init"
                    | "osal_timer_mod"
                    | "osal_timer_stop"
                    | "osal_udelay"
                    | "osal_wait_uninterruptible"
                    | "osal_wait_wakeup"
                    | "panic"
            ) =>
            {
                name
            }
            _ => "__ws63_missing_rom_callback",
        };

        generated.push_str("PROVIDE(__real_");
        generated.push_str(name);
        generated.push_str(" = ");
        generated.push_str(target);
        generated.push_str(");\n");
    }

    fs::write(output, generated).expect("write generated WS63 ROM callback fallbacks");
}

fn main() {
    println!("cargo:rustc-link-arg=-Thisi-riscv-link.x");

    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR"));
    let rf = manifest.join("../../../chips/ws63/rf/ws63-RF");
    let rf = rf.canonicalize().unwrap_or(rf);
    let lib_dir = std::env::var_os("WS63_RF_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| rf.join("lib"));
    let rom = rf.join("rom/ws63_acore_rom.lds");
    let rom_callbacks = rf.join("rom/ws63_acore_rom_callbacks.txt");
    let rom_callback_archive = rf.join("lib/librom_callback.a");

    println!("cargo:rustc-link-search=native={}", lib_dir.display());

    if std::env::var_os("CARGO_FEATURE_FULL_INIT").is_some() {
        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let exception_diag = manifest.join("src/exception_diag.S");
        let exception_diag_obj = out_dir.join("exception_diag.o");
        let status = std::process::Command::new("riscv64-unknown-elf-gcc")
            .args([
                "-x",
                "assembler-with-cpp",
                "-c",
                "-march=rv32imfc",
                "-mabi=ilp32f",
                "-o",
                exception_diag_obj
                    .to_str()
                    .expect("UTF-8 exception object path"),
                exception_diag
                    .to_str()
                    .expect("UTF-8 exception source path"),
            ])
            .status()
            .expect("run riscv64-unknown-elf-gcc for exception diagnostic");
        assert!(status.success(), "compile exception diagnostic trampoline");
        println!("cargo:rustc-link-arg={}", exception_diag_obj.display());
        println!("cargo:rerun-if-changed={}", exception_diag.display());

        let rom_fallbacks = out_dir.join("ws63_acore_rom_fallbacks.lds");
        let rom_callback_fallbacks = out_dir.join("ws63_acore_rom_callback_fallbacks.lds");
        write_rom_fallbacks(&rom, &rom_fallbacks);
        write_rom_callback_fallbacks(&rom_callbacks, &rom_callback_fallbacks);
        println!("cargo:rustc-link-arg=-T{}", rom_fallbacks.display());
        println!(
            "cargo:rustc-link-arg=-T{}",
            rom_callback_fallbacks.display()
        );
        if std::env::var_os("CARGO_FEATURE_RF_INIT_DIAG").is_some() {
            for symbol in [
                "hmac_main_init_etc",
                "wal_main_init",
                "wal_customize_set_config",
            ] {
                println!("cargo:rustc-link-arg=--wrap={symbol}");
            }
        }
        // The algorithm entry points are weak optional hooks in alg_main.c.
        // A weak undefined reference does not extract an archive member, and
        // lld otherwise resolves the call to address zero (which encodes as a
        // self-call at the call site). Seed strong undefined references and
        // visit the feature archives before the driver archive, matching the
        // vendor linker's explicit archive section selection.
        for symbol in [
            "alg_anti_intf_init",
            "alg_cca_opt_init",
            "alg_edca_opt_init",
            "alg_temp_protect_init",
            "alg_hmac_txbf_init",
            // All 37 vendor Wi-Fi mask-ROM fixes live in the same archive
            // member. Root one entry so the final ELF carries the complete,
            // internally consistent patch object; the RF post-link step then
            // discovers every `rom_symbol`/`rom_symbol_patch` pair.
            "dmac_psm_process_tim_elm_patch",
            "hh503_dispatch_trig_event_encap_patch",
        ] {
            println!("cargo:rustc-link-arg=--undefined={symbol}");
        }
        println!("cargo:rustc-link-arg=--start-group");
        for lib in [
            "wifi_alg_anti_interference",
            "wifi_alg_cca_opt",
            "wifi_alg_edca_opt",
            "wifi_alg_temp_protect",
            "wifi_alg_txbf",
            "wifi_driver_hmac",
            "wifi_driver_dmac",
            "wifi_driver_tcm",
            "bg_common",
            "wifi_rom_data",
        ] {
            if lib == "wifi_rom_data" {
                println!("cargo:rustc-link-arg=--whole-archive");
            }
            println!(
                "cargo:rustc-link-arg={}",
                lib_dir.join(format!("lib{lib}.a")).display()
            );
            if lib == "wifi_rom_data" {
                println!("cargo:rustc-link-arg=--no-whole-archive");
            }
        }
        if std::env::var_os("CARGO_FEATURE_WPA").is_some() {
            for lib in [
                "wpa_supplicant",
                "drv_security_unified",
                "hal_security_unified",
                "c",
                "m",
            ] {
                println!(
                    "cargo:rustc-link-arg={}",
                    lib_dir.join(format!("lib{lib}.a")).display()
                );
            }
        }
        // The archive contains two independent ABI payloads. Pull the complete
        // ordered veneer table and the original platform ROM-data initializer;
        // hisi-riscv-rt places the latter at the fixed DTCM addresses consumed
        // directly by mask-ROM code.
        println!("cargo:rustc-link-arg=--undefined=__wrap_log_event_wifi_print1");
        println!("cargo:rustc-link-arg=--undefined=g_systick_clock");
        println!("cargo:rustc-link-arg={}", rom_callback_archive.display());
        println!("cargo:rustc-link-arg=--end-group");
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", rom.display());
    println!("cargo:rerun-if-changed={}", rom_callbacks.display());
    println!("cargo:rerun-if-changed={}", rom_callback_archive.display());
    println!("cargo:rerun-if-changed={}", lib_dir.display());
}
