# ws63-examples — Example applications for WS63 (RISC-V) in Rust

Example applications for the HiSilicon WS63 SoC using the ws63-rs ecosystem.

## Examples

| Example | Description | Peripherals |
|---------|-------------|-------------|
| `blinky` | LED blink | GPIO |
| *(more coming)* | | |

## Building

```bash
# Configure target
export RUSTC_TARGET="hisi-riscv-rt/target-specs/riscv32imfc-unknown-none-elf.json"

# Build blinky
cargo build --release --target $RUSTC_TARGET --package blinky

# Flash (example with serial bootloader)
# ws63-flash write target/riscv32imfc-unknown-none-elf/release/blinky
```

## License

MIT
