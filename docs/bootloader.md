# Bootloader and recovery

How `BE FC` reaches the loader, what the loader is, how flashing works, and — the part that matters
for our own firmware — **how to recover a board whose application no longer answers `BE FC`.**

Evidence: the app decompilation (`re/fn.sh 0.0.18 …`), the HPMicro SDK in `re/cache/hpm_sdk/`
(partial checkout: only `boards/`, `drivers/`, `soc/` — **no `samples/`**, so the TinyUF2 sample
source is absent), and the HPM5300 datasheet. Live behaviour (re-enumeration, UF2 drive) is verified
on hardware and recorded in the task brief.

## 1. `BE FC` → jump to the loader

`BE FC 05 00 ED` dispatches to `FUN_ram_00015a0c`, which does **only this**:

1. prints `"jump to download mode ..."` (`FUN_ram_8004234e`);
2. `FUN_ram_00012a46` — mass peripheral quiesce: clears PLIC interrupt-enable bits
   (`0xe4002000`/`0xe4002004`), tears down PWM0 (`0xf0318xxx`), resets both ADCs
   (`FUN_ram_000163ce` on `0xf3080000`/`0xf3084000`);
3. `FUN_ram_800451da` — Andes CSR `0x7cc` (`mcctlcommand`) `= 7` → L1 writeback+invalidate;
4. `FUN_ram_800451b2` / `FUN_ram_800451c2` — clear bits 1 and 0 of Andes CSR `0x7ca`
   (`mcache_ctl`) → disable D-cache then I-cache;
5. `fence.i()`;
6. **direct jump**: `(*(code *)0x80020000)(0x80020000)`.

Crucially, **the app writes no "stay in bootloader" magic / backup / retention register before
jumping** — it just quiesces, flushes cache, and transfers control to `0x80020000`. So `BE FC` is a
pure warm jump into the loader. Empirically (verified) this lands in UF2 mass-storage mode, which
means the vendor loader, when entered this way, presents the UF2 drive. But that decision is made by
the **loader code we cannot see** (it is not in our dumps — see §4), not by the app.

`BE FF` (reboot, `FUN_ram_00015c9c`) is different: it arms the watchdog (`0xf410001c = 6000000`),
prints `"Waiting for WDG reset"`, and spins until the watchdog resets the chip — a cold reset, which
goes through the normal reset-time boot decision (§4), not a jump.

## 2. The loader at `0x80020000` — TinyUF2

- Live identity (verified): after `BE FC` the board re-enumerates as USB **34B7:1001 "Zent ZENT_75"**
  plus a USB Mass-Storage device **"Zent UF2 Bootloader 1.0"**. `INFO_UF2.TXT` reads
  `TinyUF2 Bootloader 0.9.0 / Model: ZENT_75 / Board-ID: ZENT_75 / Date: Sep 12 2024`.
- It is a **fork of the hpm_sdk TinyUF2 sample**. The SDK's own `flash_uf2.ld`
  (`soc/HPM5300/HPM5361/toolchains/gcc/flash_uf2.ld`) reserves `UF2_BOOTLOADER_RESERVED_LENGTH =
  0x20000` and places the app at `0x80000000 + 0x20000 = 0x80020000`. **The vendor moved the app to
  `0x80040000`** (reserving `0x40000` for the loader), so here `0x80020000` is the *loader's* entry,
  and the application starts at `0x80040000`.
- Flashing: copy a `.uf2` onto the FAT root of the mass-storage drive; the loader programs it and
  auto-resets into the app. The UF2 **family id is `0x0b4d5475`** (vendor-private). The vendor `.uf2`
  images cover only `0x80040000..0x80052900` (0.0.18) / `..0x80052200` (0.0.9), so the two config
  banks at `0x80070000`/`0x80080000` are untouched by a reflash (verified byte-identical).

## 3. The app-start signature (the key app-independent lever)

The SDK board support places a signature word at the very start of the app region. In
`boards/hpm5301evklite/board.c:76-77`:

```c
#if defined(FLASH_UF2) && FLASH_UF2
ATTR_PLACE_AT(".uf2_signature") __attribute__((used)) const uint32_t uf2_signature = BOARD_UF2_SIGNATURE;
#endif
```

and `flash_uf2.ld` puts the `.uf2_signature`/`.start` section first in FLASH. The **stock**
`BOARD_UF2_SIGNATURE` is `0x0A4D5048` ("HPM\n", `board.h:22`). The **vendor** uses **`0x0b4d5475`** —
exactly our app's word 0 at `0x80040000` and the UF2 family id. TinyUF2's `board_app_valid()` checks
this word before jumping to the app: a missing/wrong signature → the loader stays in UF2
mass-storage mode. **This is the reliable, no-hardware recovery guarantee** (see §5).

## 4. Flash access path (ROM API) and protection

All app flash I/O goes through the **ROM bootloader API table at `0x2001FF00`**
(`ROM_API_TABLE_ROOT`, `hpm_romapi.h:244`). The decompiler's `_DAT_ram_2001ff14` is
`0x2001FF00 + 0x14` = the `xpi_nor_driver_if` field (table layout `hpm_romapi.h:221-241`:
version@0, copyright@4, run_bootloader@8, otp_driver_if@0xc, xpi_driver_if@0x10,
**xpi_nor_driver_if@0x14**). Through it the app calls `*(xpi_nor_if + off)`:

- `+0x2c` = NOR read — `FUN_ram_0001598c` (`BE 01` read-flash): `(xpi_base, 4, &cfg, addr, buf, n)`.
- `+0x18` / `+0x28` = erase / program — the `BE 02` save path (`FUN_ram_00015f5a`).
- `+0x4c` / `+0x50` = status / region program (`FUN_ram_000098xx`).

First arg `&DAT_ram_f3000000` is the XPI0 flash-controller base; `&DAT_ram_0009132c` is the
`xpi_nor_config_t`. **No software block-protect** register is set by the app, so the config banks are
freely rewritten by `BE 03`+`BE 02`. The loader lives in `0x80020000..0x8003FFFF`; the app never
writes there (the `BE 01`/`BE 03`/`BE 04` address checks only ever reach `0x80070000+`), so a stock
or custom *application* cannot corrupt the loader.

## 5. Recovering a bricked board (no `BE FC`)

Our own firmware may not implement `BE FC`, so we need an entry that does not depend on the
application.

### What is app-independent and CONFIRMED

- **Invalid app signature → the loader stays in UF2 mode. CONFIRMED by device experiment
  (2026-11-24).** A copy of the stock 0.0.18 app with only word0 (`0x80040000`) changed from
  `0x0b4d5475` to `0x00000000` was packed as a UF2 (family `0x0b4d5475`, base `0x80040000`) and
  flashed. The board did **not** boot the app (no `36b0:357e`); it came straight back to the TinyUF2
  mass-storage drive (`34b7:1001` / `/dev/sda`) with no HID, no button and no host software, and
  re-flashing stock `0.0.18` restored it with the config untouched (112/112 regions == backup). This
  proves the vendor fork kept the stock signature-check mechanism (it changed the signature *value*,
  not the behaviour). Re-runnable proof: `tools/confirm_sig_recovery.py`.
- **Consequence for custom firmware.** Build custom firmware **without** a valid `0x0b4d5475` word at
  `0x80040000` and every reset lands in the loader — an app-independent recovery for development
  builds that needs no hardware. (Note the scope: this forces the loader for a *freshly flashed*
  signature-less image; it does **not** rescue an already-running signature-*valid* app that hangs —
  see "signature valid, app broken" below.) This matches the brief's "a bad app does NOT brick the
  board as long as `0x80000000..0x8003FFFF` is never written."
- Keep **never writing `0x80000000..0x8003FFFF`** from any custom app. Config writes (`0x80070000+`)
  and vendor-image flashing are safe.

### The loader's reset-time trigger — mechanism now known from the TinyUF2 source

The HPM TinyUF2 port this loader is forked from is `hpm_sdk/samples/tinyuf2` (BSD-3-Clause). Its
`check_dfu_mode()` (`src/main.c:104`) enters DFU when **either** the GPIO trigger fails **or** the
app signature is invalid:

```
if (!uf2_board_enter_bootloader()) return true;   // GPIO not "active" at reset  -> DFU
if (!uf2_board_app_valid())        return true;   // word0 != BOARD_UF2_SIGNATURE -> DFU
return false;                                      // else jump to app (APP_START+4)
```

`uf2_board_enter_bootloader()` (`src/board_api.c:159`) samples one GPIO `UF2_BOOTLOADER_SAMPLE_COUNT`
(=10) times over ~100 ms and returns "stay in app" only if the pin reads `UF2_BOOTLOADER_PIN_ACTIVE`
(=**1**, active-high) for >60 % of samples. So the pin is **normally held high → boot the app; pull it
low at reset → DFU** (matching the EVK note "PA9 to GND + reset → DFU"; the EVK uses
`BOARD_BUTTON_TINYUF2_PIN`). **Primary-source finding: this HPM port has NO double-tap / no-init-RAM
magic at all** — the ONLY triggers are this one GPIO and the signature check. `uf2_board_dfu_complete`
is `ppor_sw_reset(HPM_PPOR, 10)` (matches the observed auto-reset after flashing); the app jump is to
`APP_START+4` (matches entry `0x80040004`). **What is still unknown for THIS board:** which physical
pad the vendor fork wired as the trigger and whether it is accessible (a key/pad/test-point) — the
loader binary itself is not in our dumps (the `.uf2` starts at `0x80040000`). That one fact needs the
case opened; double-tap no longer needs investigating (ruled out by the source).

### Guaranteed hardware fallback (physical access)

From the datasheet: **BOOT_MODE[1:0] = [PA03:PA02]** sampled at reset (00 = XPI NOR app boot;
01/10 = ISP/serial boot). On **QFN48/LQFP64** the ROM ISP peripheral is **UART0 (PA00 TXD / PA01
RXD)**; the ROM **USB-HID ISP** (ROM VID `0x34B7`) exists only on **LQFP100**. **JTAG** = PA04 TDO /
PA05 TDI / PA06 TCK / PA07 TMS / PA08 TRST (may be OTP-locked). Note PA02/PA03 are the same pins the
EVK wires to its user button, so they are likely accessible near the MCU. Tools: HPM Manufacturing
Tool, `hpm_isp` (github `tfx2001/hpm_isp`), `web-hpm-isp` over UART; `riscv-openocd` (hpmicro fork),
`probe-rs`, or J-Link over JTAG. This path is independent of both the application and TinyUF2 and is
the ultimate recovery, but it requires soldering/probing pads inside the case.

### Verdict

- **Yes, without opening the case** — *if* you build the custom app so that `0x80040000` does **not**
  hold `0x0b4d5475`: the loader should refuse it and stay in UF2 mode on every reset. This is the
  recommended development safety net and does not depend on the app running.
- **Otherwise (signature valid, app broken): not guaranteed.** `BE FC` needs a running app; a
  reset-time GPIO/double-tap trigger may exist but is unconfirmed on this board.
- **Always available with physical access:** UART0 ROM ISP (PA00/PA01) with BOOT pins (PA02/PA03),
  or JTAG (PA04–PA08).

Before flashing any custom firmware: locate and label the UART0 pads, the PA02/PA03 BOOT pins and the
JTAG pads, and confirm by experiment whether a GPIO/double-tap loader trigger exists (tracked in
`open-questions.md`).

## 7. Boot chain recovered from flash dumps (2026-11-24, later)

Dumped with a temporarily flashed 0.0.18 build whose `BE 04` bank-2 read path had its source base
changed (`lui a1,0x7ff80` at ILM `0xbcbe` → `0xfff90` / `0xfff7f`; 2–3 literal bytes in the
SEGGER-LZ stream, emulation-verified as the only ILM change), read in 128-byte chunks, two identical
passes plus a known-app cross-check; stock 0.0.18 + config restored right after (`rollback.sh`).
Images (vendor code, do not publish): `re/out/loader/stage0_80000000.bin`,
`re/out/loader/loader_80020000.bin`. (`BE 01` is unusable: it hangs stock 0.0.18 even in range.)

- `0x80001000`: HPM boot header → **stage 0 at `0x80003000`** ("ZentBOOT", TinyUF2 0.9.0, built
  Sep 12 2024, vendor path `D:\ZentPro\zent75\...\soc\HPM5361\` — confirms the HPM5361).
- `0x80020000`: "Zent_DLD" download loader (what `BE FC` jumps to); its `main` goes straight to DFU.
- Stage-0 decision `0x8000c008`: if the download loader's word0 ≠ `0x0b4d5475` → stage-0's own DFU;
  else if **key check** `0x8000b6f8` → jump `0x80020004`; else if app word0 valid → jump
  `0x80040004`; else → jump download loader.
- **Key check** (`0x8000b6f8`): drives the mux address on **PY00–PY03 by software** (PGPIO
  `0xf411c1e4/1e8` set/clear; step addresses 1,2,3,4,5,9,10,11,12,13,14), ADC channels
  1,2,3,4,5,6,7,11, 5-pass average of all 88 slots. Pressed if, for **scan slot 54**,
  `d = |v−0x8000|` ≥ 2001 and `d > 3 × mean(|v−0x8000|)` over the other valid slots
  (`0x1964 ≤ v ≤ 0xe69b`). Slot 54 → key index 47 = **S** (via table `0x80040528`).
  ⇒ **Hold S while plugging in → UF2 bootloader, independent of the app.** (To confirm on hardware.)
