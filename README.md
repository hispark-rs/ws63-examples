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
| `wifi_softap` | Two-board HIL SoftAP with DHCP and bounded UDP echo | RF, UART0 |
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

For the repository-owned two-board HIL fixture, start `wifi_softap` on the AP
board and build the STA with the explicit fixture feature. This path uses
`hil_wifi_config.rs`; it does not read a developer credential file:

```bash
cargo build -Zbuild-std=core,alloc --release -p wifi_softap
cargo build -Zbuild-std=core,alloc --release \
  -p wifi_connectivity --no-default-features --features wpa2,dual-board-hil
```

The fixture is an isolated `192.168.4.0/24` network. The AP leases
`192.168.4.2`, answers UDP echo on port 9, and intentionally advertises no
default route. Public DNS is therefore skipped by contract.

Image packaging, flashing, UART capture, and secret-safe HIL orchestration live
in the parent `hisi-riscv-rs` repository. They are intentionally separate from
this repository's portable Cargo build contract.

## License

MIT
