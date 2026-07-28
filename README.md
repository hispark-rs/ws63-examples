# ws63-examples - WS63 RISC-V examples

Example applications and maintainer/HIL fixtures for the HiSilicon WS63 SoC.
The repository pins the official Rust nightly, the upstream
`riscv32imfc-unknown-none-elf` target, and the linker settings required by WS63.

## Examples

| Example | Description | Peripherals |
|---------|-------------|-------------|
| `blinky` | LED blink | GPIO |
| `uart_hello` | UART hello and tick counter | UART0 |
| `wifi_connectivity` | Public `hisi-rf` facade: scan, connect, DHCP, repeated ICMP, lease renewal | RF, UART0 |
| `rtos_*` | RTOS scheduling and Embassy conformance fixtures | Timer, software interrupt |

The parent integration repository maintains the full catalog and HIL state in
`docs/src/reference/02-examples.md`.

## Building

```bash
# rust-toolchain.toml and .cargo/config.toml select the verified toolchain/target.
cargo build -Zbuild-std=core,alloc --release -p blinky
```

The connectivity example accepts credentials only at build time. Do not commit
them:

```bash
WS63_WIFI_SSID='...' WS63_WIFI_PASSPHRASE='...' \
  cargo build -Zbuild-std=core,alloc --release \
    -p wifi_connectivity --no-default-features --features wpa2
```

Image packaging, flashing, UART capture, and secret-safe HIL orchestration live
in the parent `hisi-riscv-rs` repository. They are intentionally separate from
this repository's portable Cargo build contract.

## License

MIT
