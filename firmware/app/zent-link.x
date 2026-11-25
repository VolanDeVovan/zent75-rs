/* ZENT75-HE link script: application image for the stock TinyUF2 bootloader, all code in ILM.
 *
 * Derived from hpm-riscv-rt 0.3.1 `hpm-link.x` (MIT OR Apache-2.0) and replaces it (it is NOT
 * loaded together with it; see .cargo/config.toml). Differences from upstream:
 *
 *  1. Boot signature. Stage-0 checks that word 0 of the image (0x80040000) is 0x0B4D5475 and
 *     jumps to 0x80040004. `.app_signature` holds `APP_BOOT_SIGNATURE` (src/main.rs) at the
 *     origin and the reset code (`.init`) starts at +4. We emit no XPI boot header: the factory
 *     one at 0x80000000 must never be rewritten (docs/boot-recovery.md).
 *
 *  2. Everything runs from local memory, like the stock firmware. Only the reset stub
 *     (`.init`: set gp/sp, copy .data/.fast, call _hpm_start_rust) and the pre-init trap stay in
 *     XIP flash. ALL other code (`.text*`, the trap/vector code) goes to ILM (`.fast`), and ALL
 *     read-only data goes to DLM (inside `.data`). The startup asm copies both before any Rust
 *     code runs. Consequence: once `_hpm_start_rust` is entered the CPU never fetches from or
 *     loads from the XIP window, so erasing/programming the NOR (src/flash.rs) is safe with
 *     interrupts enabled - the 8 kHz scan keeps running during config saves - and there are no
 *     XIP cache-miss stalls in the scan ISR. `package-uf2.sh` checks that the XIP `.text` stays
 *     tiny (a regression here would make flash writes crash the firmware).
 */

ENTRY(_hpm_start);

PROVIDE(_stack_size = 0x4000);
PROVIDE(_heap_size = 0);
PROVIDE(_max_hart_id = 0);
PROVIDE(_hart_stack_size = 2K);
PROVIDE(_stack_start = ORIGIN(REGION_STACK) + LENGTH(REGION_STACK));
PROVIDE(_SEGGER_RTT = 0);
PROVIDE(__noncacheable_start__ = 0);
PROVIDE(__noncacheable_end__ = 0);

PROVIDE(InstructionMisaligned = ExceptionHandler);
PROVIDE(InstructionFault = ExceptionHandler);
PROVIDE(IllegalInstruction = ExceptionHandler);
PROVIDE(Breakpoint = ExceptionHandler);
PROVIDE(LoadMisaligned = ExceptionHandler);
PROVIDE(LoadFault = ExceptionHandler);
PROVIDE(StoreMisaligned = ExceptionHandler);
PROVIDE(StoreFault = ExceptionHandler);
PROVIDE(UserEnvCall = ExceptionHandler);
PROVIDE(SupervisorEnvCall = ExceptionHandler);
PROVIDE(MachineEnvCall = ExceptionHandler);
PROVIDE(InstructionPageFault = ExceptionHandler);
PROVIDE(LoadPageFault = ExceptionHandler);
PROVIDE(StorePageFault = ExceptionHandler);

PROVIDE(SupervisorSoft = DefaultHandler);
PROVIDE(MachineSoft = DefaultHandler);
PROVIDE(SupervisorTimer = DefaultHandler);
PROVIDE(MachineTimer = DefaultHandler);
PROVIDE(SupervisorExternal = DefaultHandler);
PROVIDE(MachineExternal = DefaultHandler);

PROVIDE(DefaultHandler = DefaultInterruptHandler);
PROVIDE(ExceptionHandler = DefaultExceptionHandler);
PROVIDE(abort = DefaultExceptionHandler);
PROVIDE(hal_main = main);
PROVIDE(__pre_init = default_pre_init);
PROVIDE(_setup_interrupts = default_setup_interrupts);
PROVIDE(_mp_hook = default_mp_hook);
PROVIDE(_start_trap = default_start_trap);

SECTIONS
{
    /* Word 0 of the application region: TinyUF2 boot signature. */
    .app_signature ORIGIN(REGION_TEXT) :
    {
        KEEP(*(.app_signature));
        ASSERT(. == ORIGIN(REGION_TEXT) + 4, "ERROR(zent): .app_signature must be exactly one word");
    } > REGION_TEXT

    /* Reset stub only (runs from XIP before the copies). Entry = 0x80040004. */
    .text ORIGIN(REGION_TEXT) + 4 :
    {
        _stext = .;
        KEEP(*(.init));
        KEEP(*(.init.rust));
        . = ALIGN(4);
        *(.trap);
        . = ALIGN(4);
        _etext_xip = .;
    } > REGION_TEXT

    /* Vector table + ALL code, executed from ILM. */
    .fast : ALIGN(512)
    {
        _sifast = LOADADDR(.fast);
        _sfast = .;
        __vector_ram_start__ = .;
        /* ILM starts at 0: entry 0 of __INTERRUPTS is CORE_LOCAL, never a null handler. */
        KEEP(*(.vector_table.interrupts));
        __vector_ram_end__ = .;
        . = ALIGN(8);
        __fast_text_start__ = .;
        *(.trap.rust);
        *(.fast .fast.text .fast.text.*);
        *(.text.abort);
        *(.text .text.*);
        . = ALIGN(4);
        __fast_text_end__ = .;
        _efast = .;
    } > REGION_FASTTEXT AT > REGION_TEXT

    __vector_load_addr__ = LOADADDR(.fast);
    __fast_text_load_addr__ = _sifast;

    /* Initialised data AND all read-only data, copied to DLM at reset. */
    .data : ALIGN(4)
    {
        _sidata = LOADADDR(.data);
        _sdata = .;
        __sdata = .;
        PROVIDE(__global_pointer$ = . + 0x800);
        *(.sdata .sdata.* .sdata2 .sdata2.*);
        *(.data .data.*);
        *(.srodata .srodata.*);
        *(.rodata .rodata.*);
        . = ALIGN(4);
        _edata = .;
        __edata = .;
    } > REGION_DATA AT > REGION_TEXT

    __sidata = LOADADDR(.data);

    .bss (NOLOAD) : ALIGN(4)
    {
        _sbss = .;
        __sbss = .;
        *(.sbss .sbss.* .bss .bss.*);
        . = ALIGN(4);
        _ebss = .;
        __ebss = .;
    } > REGION_BSS

    .fast.data : ALIGN(4)
    {
        __fast_data_start__ = .;
        *(.fast.data .fast.data.*);
        . = ALIGN(4);
        __fast_data_end__ = .;
    } > REGION_FASTDATA AT > REGION_TEXT

    __fast_data_load_addr__ = LOADADDR(.fast.data);

    .fast.bss (NOLOAD) : ALIGN(4)
    {
        __fast_bss_start__ = .;
        *(.fast.bss .fast.bss.*);
        . = ALIGN(4);
        __fast_bss_end__ = .;
    } > REGION_FASTDATA

    /* HPM53 has no PMA: "non-cacheable" data is plain DLM (DLM is not behind the D-cache). */
    .noncacheable.data : ALIGN(8)
    {
        __noncacheable_data_start__ = .;
        KEEP(*(.noncacheable.data .noncacheable.data.*));
        . = ALIGN(8);
        __noncacheable_data_end__ = .;
    } > REGION_NONCACHEABLE_RAM AT > REGION_TEXT

    __noncacheable_data_load_addr__ = LOADADDR(.noncacheable.data);

    .noncacheable.bss (NOLOAD) : ALIGN(8)
    {
        __noncacheable_bss_start__ = .;
        KEEP(*(.noncacheable .noncacheable.*));
        KEEP(*(.noncacheable.bss .noncacheable.bss.*));
        . = ALIGN(8);
        __noncacheable_bss_end__ = .;
    } > REGION_NONCACHEABLE_RAM

    .heap (NOLOAD) :
    {
        _sheap = .;
        . += _heap_size;
        . = ALIGN(4);
        _eheap = .;
    } > REGION_HEAP

    .stack (NOLOAD) :
    {
        _estack = .;
        . = ABSOLUTE(_stack_start);
        _sstack = .;
    } > REGION_STACK

    .ahb_sram (NOLOAD) :
    {
        KEEP(*(.ahb_sram .ahb_sram.*));
    } > AHB_SRAM

    .got (INFO) :
    {
        KEEP(*(.got .got.*));
    }

    .eh_frame : { KEEP(*(.eh_frame)) } > REGION_TEXT
    .eh_frame_hdr : { *(.eh_frame_hdr) } > REGION_TEXT
}

ASSERT(ORIGIN(REGION_TEXT) == 0x80040000, "ERROR(zent): the app must link at 0x80040000");
ASSERT(_stext == 0x80040004, "ERROR(zent): entry must be 0x80040004 (TinyUF2 jumps there)");
ASSERT(_etext_xip - _stext < 0x400, "ERROR(zent): code leaked into XIP .text; it must run from ILM");
ASSERT(_sdata % 4 == 0 && _edata % 4 == 0, "BUG: .data is not 4-byte aligned");
ASSERT(_sidata % 4 == 0, "BUG: LMA of .data is not 4-byte aligned");
ASSERT(_sbss % 4 == 0 && _ebss % 4 == 0, "BUG: .bss is not 4-byte aligned");
ASSERT(_stack_start - _eheap >= _stack_size, "ERROR(zent): less than _stack_size left in DLM for the stack");
ASSERT(SIZEOF(.got) == 0, "ERROR: .got section detected; dynamic relocations not supported");
