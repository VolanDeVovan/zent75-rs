# Boot chain, flashing and recovery

Read this before flashing anything. The keyboard is the owner's daily driver.

## The one hard rule

**Never write `0x80000000..0x8003FFFF`** — not by UF2, not from our firmware's flash driver. It holds
the boot header, stage-0 (with the recovery key check) and the download loader. Losing it leaves only
UART0 ROM-ISP or JTAG, which means opening the case.

`tools/flash_uf2.py` enforces this: `check_uf2()` refuses any block outside
`0x80040000..0x8006FFFF` or with a family other than `0x0B4D5475`, before touching the board
(exit code 7, no override flag). Our firmware's NOR erase/program code must range-check in the driver
itself and only ever touch the application and config regions.

## Boot chain

1. **ROM** reads the boot header at `0x80001000` and starts **stage-0** at `0x80003000`
   ("ZentBOOT", TinyUF2 0.9.0, built Sep 12 2024).
2. **Stage-0** (`0x8000C008`) decides:
   - download loader's word 0 ≠ `0x0B4D5475` → stage-0's own UF2 mode;
   - else **recovery keys held** (`0x8000B6F8`) → jump to the loader (`0x80020004`);
   - else application word 0 = `0x0B4D5475` → jump to the app (`0x80040004`);
   - else → jump to the loader.
3. **Download loader** at `0x80020000` ("Zent_DLD") always presents the UF2 drive: USB `34B7:1001`
   "ZENT_75", mass storage "Zent UF2 Bootloader", `INFO_UF2.TXT` = `TinyUF2 Bootloader 0.9.0 /
   Model: ZENT_75 / Board-ID: ZENT_75`. Copy a `.uf2` onto it → it programs and resets.
4. **Application** at `0x80040000`.

`BE FC` (app command) quiesces peripherals, flushes and disables the caches and jumps straight to
`0x80020000` — no magic word, no reset. `BE FF` arms the watchdog and spins (cold reset through
stage-0).

### Recovery key check

Stage-0 drives the mux address on PY00–PY03 (same sequence as the app, `hardware.md`), reads all 88
slots on ADC0 five times and averages. For slot 54 it computes `d = |v − 0x8000|`; the check passes
when `d ≥ 2001` and `d > 3 × mean(|v − 0x8000|)` over the other slots with `0x1964 ≤ v ≤ 0xE69B`.
A pressed key gives `d` ≈ 14700 against a resting mean ≈ 2500, so several keys held together still
pass. **On hardware: holding A+S+D while plugging in enters the loader** (confirmed 2026-11-25). S
alone did not; A vs D alone is untested.

## Recovery procedures

| Situation | What to do |
|---|---|
| App running (stock or ours, answers `BE FC`) | `tools/rollback.sh` — enters the loader, flashes stock `firmware/vendor/0.0.18.uf2`, restores the latest `backups/<ts>/` and verifies it |
| App hung / bricked / no `BE FC` | owner: unplug, **hold A+S+D**, plug in → UF2 drive appears; then `tools/rollback.sh` (it skips `BE FC` when no app is present) |
| Image without the signature | stage-0 never starts it and falls back to the loader — safe for canary builds |
| Boot region destroyed | case open: UART0 ROM-ISP (BOOT_MODE PA02/PA03, TXD/RXD PA00/PA01, e.g. `hpm_isp`) or JTAG (PA04 TDO, PA05 TDI, PA06 TCK, PA07 TMS, PA08 TRST) |

The recovery-key path needs the owner's hands; say so, it is not autonomous. Afterwards **replug with
no keys pressed**: keys held at power-up get a pressed baseline in the stock app and read travel 0
until a clean replug (`algorithms.md`).

UF2 images only touch the application region, so the config banks (`0x80070000+`) survive any
reflash byte-for-byte.

## Device-command safety

- **Never send `BE 01`** (read flash through the ROM API). It hangs stock 0.0.18 even inside its own
  allowed range; on a motherboard port only a physical replug recovers it.
- `BE 03` / `BE 04` buffers live in a 0x130-byte stack frame: keep `n` ≤ 255 bytes per frame even
  though the handler accepts up to 512.
- Do not send commands or addresses the vendor driver never sends without reading the handler first.
- Before any device write or flash: `tools/dump_all.py` (full backup of all four profiles). After:
  `tools/restore.py` verifies byte-for-byte. Leave the originally active profile selected.
- Prefer the board on the GenesysLogic hub with per-port power switching (`3-2`, `uhubctl`) over
  the motherboard port (`6-12`), so a hang can be power-cycled without the owner.

## Reading the protected region (how the boot images were obtained)

For reference only. The stock app cannot read below `0x80070000`. A 0.0.18 copy with the bank-2 read
path of `BE 04` re-based (the `lui a1, 0x7FF80` at ILM `0xBCBE` changed to `0xFFF7F`, 3 literal
bytes in the LZ stream, verified by emulating the init table) reads flash at `addr − 0x809A8` through
plain XIP loads. It was flashed temporarily, used for two identical passes, and replaced by stock
0.0.18 + config restore. Hashes of the dumps (vendor code, not committed):
stage-0 region `0x80000000+0x20000` sha256 `348b2197…aac9`,
loader `0x80020000+0x20000` sha256 `930d50ec…7bd4`.
