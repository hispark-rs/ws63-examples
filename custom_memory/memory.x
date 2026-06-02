/*
 * PER-EXAMPLE memory.x for `custom_memory`.
 *
 * Demonstrates that a binary can own its memory layout (ws63-rt's bundled
 * memory.x is disabled via `default-features = false`). The region addresses
 * here are the standard WS63 layout (so it boots), but this file — not
 * ws63-rt's — is the one the linker uses. The `__custom_memory_marker` symbol
 * below proves it at runtime: if this file is in effect the symbol resolves to
 * 0x00C0_FFEE; if it weren't, the link would fail (undefined region/symbol).
 *
 * To use a genuinely different layout, edit the MEMORY{} sizes/origins here.
 * Region NAMES must stay the same (layout.ld references FLASH/SRAM/ITCM/...).
 */

MEMORY
{
    BOOTROM  (rx) : ORIGIN = 0x100000, LENGTH = 0x9000
    ROM      (rx) : ORIGIN = 0x109000, LENGTH = 0x43000
    ITCM     (rwx): ORIGIN = 0x14C000, LENGTH = 0x4000
    DTCM     (rw) : ORIGIN = 0x180000, LENGTH = 0x4000
    FLASH    (rx) : ORIGIN = 0x200000, LENGTH = 0x800000
    PROGRAM  (rx) : ORIGIN = 0x230300, LENGTH = 0x240000
    SRAM     (rwx): ORIGIN = 0xA00000, LENGTH = 0x90000
    PRESERVE (rw) : ORIGIN = 0xA90000 - 0x100, LENGTH = 0x100
}

/* Marker proving THIS memory.x (not ws63-rt's) is in effect (see src/main.rs). */
PROVIDE(__custom_memory_marker = 0x00C0FFEE);

/* Memory regions exported as symbols for runtime relocation */
PROVIDE(__rom_start = ORIGIN(ROM));
PROVIDE(__rom_length = LENGTH(ROM));
PROVIDE(__itcm_start = ORIGIN(ITCM));
PROVIDE(__itcm_length = LENGTH(ITCM));
PROVIDE(__dtcm_start = ORIGIN(DTCM));
PROVIDE(__dtcm_length = LENGTH(DTCM));
PROVIDE(__sram_start = ORIGIN(SRAM));
PROVIDE(__sram_length = LENGTH(SRAM));
PROVIDE(__flash_start = ORIGIN(FLASH));
PROVIDE(__flash_length = LENGTH(FLASH));
PROVIDE(__program_start = ORIGIN(PROGRAM));
PROVIDE(__program_length = LENGTH(PROGRAM));

/* Stack sizes (can be overridden by user) */
__stack_size = DEFINED(__stack_size) ? __stack_size : 0x2000;
__irq_stack_size = DEFINED(__irq_stack_size) ? __irq_stack_size : 0x800;
__exc_stack_size = DEFINED(__exc_stack_size) ? __exc_stack_size : 0x800;
__nmi_stack_size = DEFINED(__nmi_stack_size) ? __nmi_stack_size : 0x400;

/* riscv-rt v0.14 required symbols */
PROVIDE(_stack_start = ORIGIN(SRAM) + LENGTH(SRAM));
PROVIDE(_max_hart_id = 0);
PROVIDE(_hart_stack_size = 0x2000);

PROVIDE(__sidata = 0);
PROVIDE(__sdata = 0);
PROVIDE(__edata = 0);
PROVIDE(__sbss = 0);
PROVIDE(__ebss = 0);

/* riscv-rt v0.14 region aliases */
REGION_ALIAS("REGION_TEXT", PROGRAM);
REGION_ALIAS("REGION_RODATA", PROGRAM);
REGION_ALIAS("REGION_DATA", SRAM);
REGION_ALIAS("REGION_BSS", SRAM);
REGION_ALIAS("REGION_STACK", SRAM);
REGION_ALIAS("REGION_HEAP", SRAM);
