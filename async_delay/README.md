# Hardware timer async example

The default example waits for five TIMER0 interrupts using HAL async without
Embassy or an RTOS. The optional `lifecycle-check` profile is a QEMU integration
regression: before enabling global interrupts, it polls/cancels finite futures,
observes selected TIMER/IRQ registers and deliberately invokes an empty IRQ
hook. It checks unpolled/zero waits, cancellation, rounding and the first chunk
of a long duration, then enables delivery and runs the original five waits.

From the parent `hisi-riscv-rs` integration workspace:

```sh
cargo build --locked -Zbuild-std=core,alloc -p async_delay \
  --features async_delay/lifecycle-check --release
uv run --script scripts/test-hal-irq-wait-qemu.py --qemu <qemu-system-riscv32> \
  --elf target/riscv32imfc-unknown-none-elf/release/async_delay \
  --lifecycle --output <new-report.json>
```

Use the model and HAL revisions recorded by the parent evidence ledger. This
does not measure physical timing or prove arbitrary ISR/WFI races. It does not
flash hardware; do not treat the opt-in profile as a new silicon acceptance test.
HAL state-machine tests live in the independently maintained HAL repository.
