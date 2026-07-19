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

fn write_exception_diag_object(output: &Path) {
    use object::write::{Object, Relocation, Symbol, SymbolSection};
    use object::{
        Architecture, BinaryFormat, Endianness, FileFlags, RelocationFlags, SectionKind,
        SymbolFlags, SymbolKind, SymbolScope,
    };

    let mut object = Object::new(BinaryFormat::Elf, Architecture::Riscv32, Endianness::Little);
    object.flags = FileFlags::Elf {
        os_abi: 0,
        abi_version: 0,
        // EF_RISCV_RVC | EF_RISCV_FLOAT_ABI_SINGLE.
        e_flags: 0x3,
    };
    let text = object.add_section(
        Vec::new(),
        b".text.default_exc_handler".to_vec(),
        SectionKind::Text,
    );
    // auipc t1, 0; jalr x0, t1, 0. R_RISCV_CALL_PLT resolves the pair to the
    // Rust diagnostic function at final link time. Keeping this as a separate
    // ELF object lets the strong symbol override the runtime's weak handler
    // without merging duplicate global_asm definitions under LTO.
    object.append_section_data(text, &[0x17, 0x03, 0x00, 0x00, 0x67, 0x00, 0x03, 0x00], 4);
    object.add_symbol(Symbol {
        name: b"default_exc_handler".to_vec(),
        value: 0,
        size: 8,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Section(text),
        flags: SymbolFlags::None,
    });
    let target = object.add_symbol(Symbol {
        name: b"__ws63_rf_exception_diag".to_vec(),
        value: 0,
        size: 0,
        kind: SymbolKind::Text,
        scope: SymbolScope::Linkage,
        weak: false,
        section: SymbolSection::Undefined,
        flags: SymbolFlags::None,
    });
    object
        .add_relocation(
            text,
            Relocation {
                offset: 0,
                symbol: target,
                addend: 0,
                // R_RISCV_CALL_PLT applies to the AUIPC/JALR instruction pair.
                flags: RelocationFlags::Elf { r_type: 19 },
            },
        )
        .expect("add exception diagnostic relocation");
    fs::write(
        output,
        object
            .write()
            .expect("serialize exception diagnostic object"),
    )
    .expect("write exception diagnostic object");
}

fn metadata_list(name: &str) -> Vec<String> {
    std::env::var(name)
        .unwrap_or_else(|_| panic!("ws63-radio-sys did not export {name}"))
        .split(',')
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
        .collect()
}

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

    let lib_dir = std::env::var_os("WS63_RF_LIB_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            PathBuf::from(
                std::env::var_os("DEP_WS63_RADIO_SYS_LIB_DIR")
                    .expect("ws63-radio-sys did not export its archive directory"),
            )
        });
    let rom = PathBuf::from(
        std::env::var_os("DEP_HISI_ROM_SYS_WS63_ROM_SYMBOLS")
            .expect("hisi-rom-sys did not export WS63 ROM symbols"),
    );
    let rom_callbacks = PathBuf::from(
        std::env::var_os("DEP_HISI_ROM_SYS_WS63_ROM_CALLBACKS")
            .expect("hisi-rom-sys did not export WS63 ROM callbacks"),
    );
    let rom_callback_archive = PathBuf::from(
        std::env::var_os("DEP_WS63_RADIO_SYS_ROM_CALLBACK_ARCHIVE")
            .expect("ws63-radio-sys did not export its ROM callback archive"),
    );
    let nvs_linker = PathBuf::from(
        std::env::var_os("DEP_WS63_RADIO_SYS_NVS_LINKER")
            .expect("ws63-radio-sys did not export its NVS linker contract"),
    );

    println!("cargo:rustc-link-search=native={}", lib_dir.display());
    println!("cargo:rustc-link-arg=-T{}", nvs_linker.display());
    println!("cargo:rerun-if-changed={}", nvs_linker.display());

    if std::env::var_os("CARGO_FEATURE_FULL_INIT").is_some() {
        let out_dir = PathBuf::from(std::env::var_os("OUT_DIR").expect("OUT_DIR"));
        let exception_diag = out_dir.join("exception_diag.o");
        write_exception_diag_object(&exception_diag);
        println!("cargo:rustc-link-arg={}", exception_diag.display());

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
        // The profile owns optional algorithm roots and the complete mask-ROM
        // patch archive roots; the example does not duplicate blob ABI facts.
        for symbol in metadata_list("DEP_WS63_RADIO_SYS_WIFI_ROOT_SYMBOLS") {
            println!("cargo:rustc-link-arg=--undefined={symbol}");
        }
        println!("cargo:rustc-link-arg=--start-group");
        for archive in metadata_list("DEP_WS63_RADIO_SYS_WIFI_ARCHIVES") {
            let (name, mode) = archive
                .split_once(':')
                .expect("invalid ws63-radio-sys archive metadata");
            if mode == "whole" {
                println!("cargo:rustc-link-arg=--whole-archive");
            }
            println!(
                "cargo:rustc-link-arg={}",
                lib_dir.join(format!("lib{name}.a")).display()
            );
            if mode == "whole" {
                println!("cargo:rustc-link-arg=--no-whole-archive");
            }
        }
        if std::env::var_os("CARGO_FEATURE_PERSONAL").is_some() {
            for lib in metadata_list("DEP_WS63_RADIO_SYS_WPA_ARCHIVES") {
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
        for symbol in metadata_list("DEP_WS63_RADIO_SYS_ROM_CALLBACK_ROOT_SYMBOLS") {
            println!("cargo:rustc-link-arg=--undefined={symbol}");
        }
        println!("cargo:rustc-link-arg={}", rom_callback_archive.display());
        println!("cargo:rustc-link-arg=--end-group");
    }

    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed={}", rom.display());
    println!("cargo:rerun-if-changed={}", rom_callbacks.display());
    println!("cargo:rerun-if-changed={}", rom_callback_archive.display());
    println!("cargo:rerun-if-changed={}", lib_dir.display());
    println!("cargo:rerun-if-env-changed=CARGO_FEATURE_PERSONAL");
}
