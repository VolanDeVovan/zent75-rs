# AGENTS.md — rules for working on the ZENT75-HE

The keyboard is the owner's **daily driver**. Every device action must leave it recoverable.
Background and evidence: `README.md`, `docs/bootloader.md` (§5, §7), `docs/protocol.md`.

## Flash layout and the one hard rule

| Region | Contents | May we write it? |
|---|---|---|
| `0x80000000..0x80002FFF` | XPI NOR config + HPM boot header | **NEVER** |
| `0x80003000..0x8001FFFF` | stage-0 ("ZentBOOT"): decides app / loader, holds the **A+S+D recovery check** | **NEVER** |
| `0x80020000..0x8003FFFF` | download loader ("Zent_DLD", TinyUF2 drive, target of `BE FC`) | **NEVER** |
| `0x80040000..0x8006FFFF` | application (word0 = `0x0B4D5475` boot signature) | yes — via UF2 |
| `0x80070000..` | config banks (top/bot, keymaps, profiles…) | only via `BE 03` + `BE 02` |

**Never write `0x80000000..0x8003FFFF`** — not by UF2, not from our firmware's flash driver, not
"just once". It holds the only app-independent recovery path; losing it means opening the case and
using UART0 ISP (PA00/PA01) or JTAG (PA04–PA08).

- `tools/flash_uf2.py` enforces this: `check_uf2()` refuses any block outside the application
  region before the board is touched (exit code 7). There is intentionally no override flag — do
  not add one, and do not copy `.uf2` files to the bootloader drive by hand to get around it.
- Our firmware's NOR erase/program code must hard-limit itself to the application and config
  regions (assert the address range in the driver, not only at call sites).

## Recovery (verified on hardware 2026-11-25)

1. **Firmware hung / bricked app:** unplug, **hold A+S+D**, plug in → UF2 drive `34b7:1001`
   appears (stage-0 check, independent of the app). Then run `tools/rollback.sh`: with no app HID
   present it skips `BE FC`, flashes stock `firmware/vendor/0.0.18.uf2`, restores and verifies the
   latest `backups/<ts>/`. Step 1 needs the owner's hands — say so plainly; it is not autonomous.
2. **App running:** `tools/rollback.sh` alone (enters the loader with `BE FC`).
3. **Signature-less image** (word0 ≠ `0x0B4D5475`): the loader refuses it and stays in UF2 mode —
   always safe for canary builds.
4. Last resort, case open: UART0 ROM ISP or JTAG (see `docs/bootloader.md` §5).

After any recovery test: tell the owner to **replug with no keys pressed**. Keys held while the
board powers up get a pressed baseline and their travel sticks at 0 until a clean replug (this is
also the cause of the "travel stuck at 0" bug — see `docs/algorithms.md`).

## Device-command rules

- **Never send `BE 01`** (read flash). It hard-hangs stock 0.0.18 even inside its own allowed
  range; with the board on a motherboard port only a physical replug recovers it.
- Read the handler in the disassembly before sending any command or address the vendor driver
  does not send. No exploratory probing of the live device.
- `BE 04` / `BE 03` requests: at most 255 data bytes per frame (the handler's reply buffer sits in a
  0x130-byte stack frame).
- Before any device write or flash: fresh backup with `tools/dump_all.py`; afterwards verify with
  `tools/restore.py` (byte-identical, all four profiles) and leave the originally active profile
  selected.
- Prefer the board on the GenesysLogic hub with per-port power switching (`3-2`) so a hang can be
  power-cycled with `uhubctl`; on the motherboard port (`6-12`) it cannot.

## Before flashing a signature-valid custom build

- Build with `firmware/package-uf2.sh` (app base `0x80040000`, family `0x0B4D5475`).
- Confirm `check_uf2` passes and the image does not contain NOR erase/program paths that can reach
  `0x80000000..0x8003FFFF`.
- Tell the owner beforehand that a hang means "hold A+S+D and replug", and have `rollback.sh` ready.
