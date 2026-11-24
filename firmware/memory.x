/* Memory map for the HOVEY WORKS ZENT75-HE (HPM5361, RV32IMAFDC). See docs/hardware.md.
 *
 *   0x80000000..0x8003FFFF : boot header, stage-0 (recovery), download loader.
 *                            *** MUST NEVER BE WRITTEN *** (docs/boot-recovery.md).
 *   0x80040000..0x8006FFFF : our application image. Stage-0 checks word 0 (boot
 *                            signature 0x0B4D5475) and jumps to 0x80040004.
 *   0x80070000..           : config banks (shared with the stock firmware).
 *
 * So REGION_TEXT starts at 0x80040000 and the very first word of the image must
 * be the signature (see link-app.x, section `.app_signature`). We do NOT emit an
 * XPI NOR boot header of our own; the factory one at 0x80000000 already exists.
 */
MEMORY
{
    /* Application image region (tools/flash_uf2.py refuses anything outside it). */
    XPI0_APP    : ORIGIN = 0x80040000, LENGTH = 0x80070000 - 0x80040000 /* 0x30000 = 192K, stops at the config banks */
    ILM         : ORIGIN = 0x00000000, LENGTH = 128K /* instruction local memory */
    DLM         : ORIGIN = 0x00080000, LENGTH = 128K /* data local memory */
    AHB_SRAM    : ORIGIN = 0xf0400000, LENGTH = 32K
}
REGION_ALIAS("REGION_TEXT", XPI0_APP);
REGION_ALIAS("REGION_FASTTEXT", ILM);
REGION_ALIAS("REGION_FASTDATA", DLM);
REGION_ALIAS("REGION_RODATA", XPI0_APP);
REGION_ALIAS("REGION_DATA", DLM);
REGION_ALIAS("REGION_BSS", DLM);
REGION_ALIAS("REGION_HEAP", DLM);
REGION_ALIAS("REGION_STACK", DLM);
/* PMA is not available in HPM53 devices, keep this as a placeholder */
REGION_ALIAS("REGION_NONCACHEABLE_RAM", DLM);
