/* ZENT75-HE application image layout for the stock TinyUF2 bootloader.
 *
 * The bootloader validates that the first 32-bit word of the image at
 * 0x80040000 is the boot signature 0x0B4D5475, then jumps to the entry at
 * 0x80040004 (verified against firmware/vendor/0.0.18.uf2: word0 = 0x0B4D5475,
 * word1 = first instruction `lui gp,0x92`).
 *
 * hpm-riscv-rt's hpm-link.x places the reset handler (`.init`) first in `.text`
 * at `_stext`, and `PROVIDE(_stext = ORIGIN(REGION_TEXT))`. We:
 *   1. Shift `_stext` to ORIGIN+4 so the reset handler lands at 0x80040004.
 *      A plain assignment overrides the PROVIDE regardless of script order.
 *   2. Insert a `.app_signature` output section (holding the signature word)
 *      at ORIGIN(REGION_TEXT) = 0x80040000, ahead of everything else in the
 *      region. It is inserted before `.text.dummy` (the NOLOAD anchor that
 *      hpm-link.x uses to realize `_stext`): with `_stext` at ORIGIN+4, the
 *      dummy then lands at ORIGIN+4 with zero size, and `.text` follows it,
 *      so nothing overlaps the signature word.
 *
 * We do NOT emit any XPI NOR boot header: the factory one at 0x80000000 is
 * already present and must never be rewritten.
 */

_stext = ORIGIN(REGION_TEXT) + 4;

SECTIONS
{
    .app_signature ORIGIN(REGION_TEXT) :
    {
        KEEP(*(.app_signature));
    } > REGION_TEXT
} INSERT BEFORE .text.dummy;
