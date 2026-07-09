# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Changed

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
