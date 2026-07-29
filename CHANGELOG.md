# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.57` and replaced its
  process-global runner/wait diagnostic cells with the facade-owned,
  task-split-safe unified snapshot.
- Extended the post-ping diagnostic marker through the complete v4 data-path
  chain: smoltcp TX, vendor bridge TX, DMAC completion, vendor/Rust RX, MAC
  receive counters, and IRQ dispatches.

### Added

- Added `wifi_connectivity`, the public `hisi-rf 0.1.0-alpha.48` end-to-end
  example covering the incremental runner, scan/connect, smoltcp DHCP, repeated
  ICMP, and lease renewal.
- Added a pinned official nightly and a repository-local Cargo target/linker
  contract so this release unit builds independently of the parent workspace.

### Changed

- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.49` and moved
  profile-specific crypto peripheral ownership into the example's typestate
  resource builder. WPA2 no longer consumes PKE; WPA3 requires it before
  resources can be built.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.52` and replaced the
  public control-storage plus arena pair with one `declare_radio_storage!`
  composition and admission step. Its host-side resource report now uses the
  WS63 RV32 layout rather than the build host's pointer width.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.53`; event capacity is now
  owned by the selected profile and no longer appears in application control
  types or the caller-owned storage declaration.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.54`, moved all
  user-facing radio, operation, application-wait, credential, and runner
  settings into its local `config` module, and adopted distinct typed
  operation/backend timeout contracts. The older compatibility smoke was
  migrated to the same public timeout API.
- Updated `wifi_connectivity` to `hisi-rf 0.1.0-alpha.55`; the smoltcp runner
  now obtains its station MAC from its own initialized `WifiDevice` instead of
  a process-global netif accessor.
- Made the embedded release profile explicit (`opt-level = "s"`, LTO, debug
  symbols, and one codegen unit), preventing parent-workspace profile drift from
  changing the WS63 SRAM/link layout.
- Migrated every example from the retired `hisi-riscv-hal` package and
  `hisi_riscv_hal` import path to `hisi-hal 0.7.0-alpha.1` / `hisi_hal`.
- **dma_loopback** — retargeted part 2 (mem->mem) from the secure DMA (SDMA
  @0x520A_0000) to the primary M_DMA channel 1. SDMA is never provisioned on WS63
  silicon — a transfer there stalls AXI and hangs the bus — so the example no
  longer exercises it (matches the silicon-faithful `ws63-qemu` DMA model).

### Added
- **xip_flash_clk_hazard** — demonstrates the issue-#4 hazard: re-switching the flash clock (CLDO_CRG_CLK_SEL bit 18) while executing XIP from flash crashes instruction fetch; ws63-qemu now faults it

- **uart_hello** — UART0 serial print example (QEMU-friendly)
- **timer_irq** — TIMER_0 interrupt (IRQ 26) handling example
- **gpio_irq** — GPIO0 pin0 interrupt (IRQ 33) example with custom local IRQ >=32
- **reset_demo** — System reset example (software_reset + reset_reason)
- **dma_loopback** — Peripheral DMA mem<->SPI0 loopback + mem->mem, both on the primary M_DMA
- **wifi_blob_link** — Wi-Fi ROM blob linking spike using hisi-riscv-rt's `.wifi_pkt_ram` symbols
- **rf_port_demo** — ws63-rf-rs porting layer + blob link exercise
- **sched_demo** — ws63-rf-rs cooperative scheduler validation (later moved to ws63-rf-rs)
- **blinky** build.rs — Automatic hisi-riscv-rt linker script discovery (-Tws63-link.x)

### Changed

- **timer_irq, gpio_irq** — Refactored to use hisi_riscv_hal::interrupt controller API
- **wifi_blob_link examples** — Point at nested ws63-RF (ws63-rf-rs/ws63-RF)

### Fixed

- **clippy** — Fixed fn_to_numeric_cast warning in trap-handler (cast through raw pointer)

### Removed

- **sched_demo** — Moved to ws63-rf-rs as an internal example

## [0.1.0]

### Added

- Initial ws63-examples repository with blinky LED example
- **blinky** — GPIO output and busy-wait delay demonstration
  - Uses `hisi-riscv-rt::entry` for startup
  - Uses `hisi-riscv-hal::gpio::create_output_pin` for GPIO control
  - Demonstrates minimal `#![no_std]` + `#![no_main]` embedded application pattern
- Project documentation (ARCHITECTURE.md, README.md)
- Workspace Cargo configuration with path dependencies (ws63-pac, hisi-riscv-hal, hisi-riscv-rt)
