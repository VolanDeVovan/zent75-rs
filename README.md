# zent75-he

Reverse engineering of the **HOVEY WORKS ZENT75-HE** (sold as "Wob Lab ZENT 75 HE") hall-effect
keyboard. The goal is our own configurator panel, and eventually our own firmware (Rust if possible),
in place of the vendor's Chinese web driver.

## Links

- Vendor web driver (WebHID, Chrome/Edge only): https://hoveyworks-driver.com/
  - main bundle: `https://hoveyworks-driver.com/js/app.<hash>.js` (protocol, address map, keymap codes)
  - firmware: `https://hoveyworks-driver.com/firmware/zent75/master/{0.0.9,0.0.18}.uf2`.
    The driver's dropdown calls 0.0.9 "稳定版本" (stable) and 0.0.18 "最新版本" (latest).
- Not this keyboard: WOBHub (https://www.wobwxe.com/) is a different vendor tool. Its PID list does not include ours.

## Device

| | |
|---|---|
| USB | `36b0:357e`, product "ZENT  75 HE", manufacturer "Wob Lab" |
| Firmware on the unit | 0.0.18 (as of 2026-11-24) |
| Keys | 83 (the driver's `keyboardLength`) |
| Polling | 8 kHz advertised |
| Interfaces | if00 boot keyboard (hidraw2), if01 consumer control (hidraw6), **if02 vendor raw HID** (hidraw7, usage page `0xFF60`/usage `0x61`, 1024-byte reports, report id 0), if03 mouse (hidraw8) |

On the desktop the udev rule in `nixos/parts/modules/nixos/hardware/keyboard-zent75.nix` makes every
`36b0` hidraw node mode 0666.

## Protocol (stock firmware, from the vendor JS)

Each frame is `BE <cmd> <len LE16> ... ED`, padded to 1024 bytes and sent as report 0.

| Command | Frame | Notes |
|---|---|---|
| Heartbeat | `BE 00 05 00 ED` | While it keeps arriving (≈1/s) the board streams `BE FD` frames |
| Read flash | `BE 01 0B 00 <addr LE32> <len BE16> ED` | |
| Save RAM → flash | `BE 02 05 00 ED` | |
| Write RAM | `BE 03 <13+n LE16> <addr LE32> <n BE16> <data…> <crc LE16> ED` | ack `BE 03 05 00 ED` |
| Read RAM | `BE 04 0B 00 <addr LE32> <len BE16> ED` | reply `BE 04 <n LE16>`, data at `[10:n-3]`, crc at `[n-3:n-1]` |
| Serial number | `BE 1D 05 00 ED` | reply `BE 1D 15 00 …` |
| Switch profile | `BE C0 06 00 <0-3> ED` | the board also pushes `BE C0 06 00 <n>` on an Fn switch |
| Reset config | `BE EE 05 00 ED` | |
| Bootloader | `BE FC 05 00 ED` | firmware is then delivered as `.uf2` |
| Calibration | `BE FE 06 00 <1 on / 0 off> ED` | |
| Reboot | `BE FF 05 00 ED` | |

The CRC is CRC-16/MODBUS (poly `0xA001` reflected, init `0xFFFF`) over everything before it.

The `BE FD` stream has length 1017: 83 raw ADC `u16`, then 83 travel `u16` in µm. A 1003-length
variant carries 6 × 83 raw samples. The ADC rests at ≈30000 and drops to ≈18000 at bottom-out.

### RAM map (83 entries × u16 unless noted)

| Name | Address | Len |
|---|---|---|
| top_value / bot_value (calibration) | `0x80070000` / `0x800700B0` | 166 / 166 |
| keytype_code | `0x80070160` | 166 |
| soft_version | `0x80070210` | 4 |
| cur_param_num (profile) | `0x80070218` | 4 |
| config_name1..4 | `0x80070290` + 64·i | 64 |
| key_map / key_map_fn | `0x80080160` / `0x80080210` | 166 / 166 |
| press_th / return_th | `0x800802C0` / `0x80080370` | 166 / 166 |
| press_deadzone / return_deadzone | `0x80080420` / `0x800804D0` | 166 / 166 |
| norm_press_th / norm_return_th | `0x80080580` / `0x80080630` | 166 / 166 |
| key_mode (u8) | `0x800806E0` | 83 |
| key_cancel_code (SOCD "后覆盖") | `0x80080A50` | 166 |
| rgb / rgb_solid_enable / rgb_config | `0x80080B00` / `0x80080C08` / `0x80080C60` | 255 / 85 / 5 |
| rgb_enable, cap_enable, rgb_fn_enable | `0x80080C68`, `0x80080C69`, `0x80080C6B` | 1 each |
| sleep_second_cnt | `0x80080C6C` | 4 |
| win_lock_state, auto_calib_enable | `0x80080C70`, `0x80080C71` | 1 each |
| bind_key_setting (DKS / groups, 48 B per key) | `0x80080D00` + 48·key | 48 |
| bind_key_mode (u8) | `0x80081DE0` | 83 |

Keymap codes are signed `i16`: modifiers -8..-1 (LCtrl…RWin), HID usages 4–119, consumer 201–225,
board functions 301–320 (319 = 5 s reset), FN layers 401–404, mouse 251–255, 1000 = empty. Key
indices go row by row: Esc = 0, LShift = 59, LCtrl = 73, RCtrl = 79, Right = 82.

## Layout

- `firmware/vendor/`: vendor `.uf2` images, untouched; `firmware/` will hold our Rust project (vendor binaries, do not publish)
- `tools/`: Python scripts from the first debugging session
  - `zent_read.py`: read-only dump of firmware version, profile, both layers and key modes
  - `zent_all.py`: 30 s live stream that flags keys whose sensor moves while the firmware sees ~0 travel
  - `zent_calib.py`: per-key recalibration that mirrors the vendor flow and writes **to the device**
  - `zent_calib_backup_*.json`: top/bot values from before that calibration (all 31000/17000)

## History

- 2026-11-24: LCtrl/LShift went dead (sensor fine, firmware travel ~0). LShift came back after a
  recalibration and a replug, LCtrl only after a second replug. Indices 59/73 now hold measured
  calibration instead of 31000/17000.

## Project status (2026-11-24)

Reverse-engineering, a configurator panel, and a firmware plan are done; custom-firmware flashing is
intentionally still gated (see below). The keyboard is on stock 0.0.18 with the user's config intact.

- **MCU**: HPMicro **HPM5361** (QFN48, part HPM5361IEG1 / HPM53x1IEG1), RV32IMAFDC, XIP flash @0x80000000,
  TinyUF2 bootloader @0x80020000, application @0x80040000 (boot signature word0 = `0x0B4D5475`).
- **docs/**: `hardware.md`, `protocol.md`, `algorithms.md`, `bootloader.md`, `stable-vs-latest.md`,
  `open-questions.md`, `firmware-plan.md` — all evidence-cited to function addresses in the disassembly.
- **re/**: re-runnable analysis (`uf2.py`, `segger_init.py`, Ghidra headless `ghidra/run.sh`, `fn.sh`,
  function matcher). Ghidra output under `re/out/` is gitignored.
- **tools/**: `zentlib.py` (+`rammap.py`) raw-HID library, `dump_all.py`, `restore.py`, `flash_uf2.py`,
  `rollback.sh` (autonomous recovery), unit tests in `tools/tests/` (48 pass).
- **backups/**: full config dump of all 4 profiles with sha256 + the verified rollback log.
- **panel/**: TypeScript + Vite WebHID configurator (97 tests pass; verified reading, editing, saving
  and restoring on the real keyboard via the shared codec).
- **firmware/**: compiling Rust skeleton (hpm-hal/embassy, `riscv32imafc`) + `vendor/` stock images.

### Rollback gate — PASSED (autonomous, no replug)
`BE FC` → TinyUF2 mass-storage (USB `34B7:1001`, `/dev/sda`) → flash a `.uf2` → auto-reboot to app.
Vendor images touch only `0x80040000..0x80052900`, so config banks survive a reflash.
`tools/rollback.sh` does flash-stock + restore-config + verify in one command.

### Flashing custom firmware is GATED
A running-but-hung custom app can't answer `BE FC`, and the app-independent TinyUF2 reset trigger
(GPIO / double-tap) is unconfirmed without opening the case (see `docs/open-questions.md`). A
signature-less *canary* build (omit word0) is auto-rejected by the loader and always recoverable —
safe for development only. Do not flash a signature-valid custom build until recovery is confirmed.
