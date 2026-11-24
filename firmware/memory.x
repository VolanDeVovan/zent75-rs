/* Memory map for the HOVEY WORKS ZENT75-HE (HPM5300-family, RV32IMAFDC).
 *
 * This board ships a TinyUF2 bootloader, NOT the stock "boot header at base +
 * app right after" layout the hpm-hal example uses. On this board:
 *
 *   0x80000000..0x8003FFFF : factory XPI boot header + TinyUF2 bootloader.
 *                            *** MUST NEVER BE WRITTEN *** (our recovery path).
 *   0x80040000..           : our application image. TinyUF2 validates the first
 *                            32-bit word (boot signature 0x0B4D5475) and jumps
 *                            to the entry at 0x80040004.
 *
 * So REGION_TEXT starts at 0x80040000 and the very first word of the image must
 * be the signature (see link-app.x, section `.app_signature`). We do NOT emit an
 * XPI NOR boot header of our own; the factory one at 0x80000000 already exists.
 */
MEMORY
{
    /* Application image region accepted by the stock TinyUF2 bootloader. */
    XPI0_APP    : ORIGIN = 0x80040000, LENGTH = 0x80100000 - 0x80040000 /* 0xC0000 = 768K */
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
