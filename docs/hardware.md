# Hardware map

Track A reverse engineering of the HOVEY WORKS ZENT75-HE (USB `36b0:357e`). All addresses are from
the 0.0.18 decompilation unless noted. Function names are Ghidra labels; run
`re/fn.sh 0.0.18 <hexaddr>` to see any `FUN_ram_*`. Peripheral base addresses were matched against
`re/cache/hpm_sdk/soc/HPM5300/HPM5361/HPM5361_svd.xml`.

## MCU

**HPMicro HPM5361 (HPM5300 family), almost certainly in the QFN48 package.**

Evidence:

- **Vendor is HPMicro, family HPM5300.** The firmware is built on the `hpm_sdk`: strings
  `rgb_matrix_init_drivers` (QMK), `cherry usb hid_keyboard device sample`, `[I/USB]`/`[E/USB]`
  (CherryUSB), `jump to download mode ...`, `SPI master timing init failed`. Every peripheral
  register address the code touches resolves cleanly in the HPM5300 SVD (table below).
- **Two 16-bit ADCs → not the single-ADC HPM5301.** The code drives both `ADC0` (`0xf3080000`)
  and `ADC1` (`0xf3084000`) — see `FUN_ram_0001023c` (ADC init) and `FUN_ram_000002a0` /
  `FUN_ram_000003a6` (the two DMA-complete handlers). In the SDK, only HPM5361 and HPM53M1 expose
  `HPM_ADC1_BASE`; HPM5301 has `ADC0` only
  (`re/cache/hpm_sdk/soc/HPM5300/HPM5301/hpm_soc_ip.h` has no `ADC1`).
- **QFN48 package (8 analog inputs).** The scan samples 4 ADC channels on each ADC (8 analog pins
  total — see "Hall-sensor readout"), which is exactly the ADC-pin budget of the QFN48 part
  ("only 8 on QFN48" per the datasheet; LQFP64/LQFP100 expose up to 16). 83 keys are therefore read
  through external analog multiplexers, as on the Rainy75-HE sibling.
- **128 KB ILM + 128 KB DLM.** `_start` sets `sp = 0x000A0000` (`re/segger_init.py`), the top of a
  128 KB DLM based at `0x80000`; the SDK linker scripts confirm `ILM = 128k @ 0x0`,
  `DLM = 128k @ 0x80000` (`.../HPM5361/toolchains/segger/flash_uf2.icf:12-13`). This matches
  HPM5361/HPM53M1, not the smaller HPM5301.

**HPM5361 vs HPM53M1 cannot be told apart from the binary**: their SDK peripheral maps
(`hpm_soc_ip.h`, `hpm_soc_ip_feature.h`) are byte-identical except for a copyright line, so the
firmware reads and writes the same registers on either die. HPM5361 is the mainstream 2-ADC part
and the only one with full board support in this SDK checkout, so it is the identification; reading
the physical package marking is the only way to be 100 % certain (see `open-questions.md`).

### Core / ISA

RISC-V **RV32IMAFDC** (HPM5300 core is the Andes D25, single 480 MHz hart with FPU). The decompiler
shows hardware single-/double-precision float calls in the calibration path (`FUN_ram_80042830`
etc.), `fence.i` in the bootloader jump (`FUN_ram_00015a0c`), and 16-bit compressed opcodes
throughout — consistent with IMAFDC.

## Memory and configuration layout

| Region | Address | Notes |
|---|---|---|
| XIP NOR flash | `0x80000000` | external QSPI NOR, executed in place (XPI0) |
| TinyUF2 bootloader | `0x80020000` | jump target of `BE FC`; see `bootloader.md` |
| Application | `0x80040000` | word0 = `0x0b4d5475` ("uTM\x0b") boot signature, entry `0x80040004` |
| App image extent | `0x80040000..0x80052900` | 0.0.18 (76032 B); 0.0.9 ends at `0x80052200` |
| ILM (code, copied at boot) | `0x00000000`, 128 KB | SEGGER init table copies/decompresses here (`re/segger_init.py`) |
| DLM (data/stack) | `0x00080000`, 128 KB | `sp = 0xA0000` |

The stock `hpm_sdk` TinyUF2 sample reserves `0x20000` and starts the app at `0x80020000`
(`flash_uf2.icf`); the vendor moved the app to `0x80040000`, i.e. reserved `0x40000` for the loader.

### Config "RAM" and its shadows

The vendor driver's `BE 03`/`BE 04` "RAM" addresses are not real RAM addresses — the firmware
translates them to in-RAM shadow copies by adding a constant (dispatcher `FUN_ram_0000b812`):

| Vendor bank | Address range | Translation | Lands at (RAM) | Contents |
|---|---|---|---|---|
| Bank 1 (board-global) | `0x80070000 .. 0x80078000` | `+0x7ff97084` | `0x00007084` (ILM) | top/bot calibration, keytype, version, active profile, 4 profile names |
| Bank 2 (active profile) | `0x80080000 .. 0x80100000` | `+0x7ff80658` | `0x00000658` (ILM) | keymap, thresholds, modes, SOCD, RGB, DKS/groups |
| Bank 2 "live" mirror | `0x80080000 ..` | `+0x7ff8f814` | `0x0000f814` (ILM) | a second copy of some bank-2 data written on RGB/live updates |

So e.g. `top_value` (`0x80070000`) is `DAT_ram_00007084`, `bot_value` (`0x800700B0`) is
`DAT_ram_00007134`, `keytype_code` (`0x80070160`) is `DAT_ram_000071e4`, and
`auto_calib_enable` (`0x80080C71`) is `DAT_ram_000012c9` (`0x658 + 0xC71`). The scan/algorithm code
reads these shadows directly. `BE 01` ("read flash") instead adds `+0x80000000`, which maps
`0x80080000→0x00080000` = raw DLM; it is a memory peek, never used by the driver, and unsafe
(see `protocol.md`).

## Hall-sensor readout

The 83 analog Hall sensors are read with **both 16-bit ADCs, hardware-triggered, DMA to RAM, through
external analog multiplexers**, at an **8 kHz** full-scan rate.

- **ADC setup** — `FUN_ram_0001023c`. Both `ADC0` (`0xf3080000`) and `ADC1` (`0xf3084000`) are
  initialised with a 16-entry conversion sequence. The channel list is a const table at
  `0x800422b4`: channels `1,3,5,7,0,2,4,6` (plus `11`), each listed 4× — i.e. **4 analog channels
  per ADC, each oversampled 4 conversions**. Bus-mode is enabled by setting bit `0x800000` in the
  ADC config register (`0xf3081114`/`0xf3085114`). The sequence is hardware-triggered by **PWM0**
  (`0xf0318000`) routed through **TRGM0** (`DAT_ram_f033c180 = DAT_ram_f033c184 = 0x30`), and each
  ADC DMAs its results to a 64-byte buffer: ADC0 → `0x00092260`, ADC1 → `0x00092160`.
- **DMA-complete handlers** — `FUN_ram_000002a0` (ADC0), `FUN_ram_000003a6` (ADC1). Each copies its
  0x40-byte result block (16 × u32) into a growing per-scan buffer (ADC0 → `0x0008d408`,
  ADC1 → `0x0008d148`), advancing `0x40` each time, and sets a per-step "done" bit in
  `DAT_ram_000920b8` (bit 0 = ADC0, bit 1 = ADC1). When both are done for a step it marks that
  step valid in the 11-entry flag array at `DAT_ram_00091d98` and calls `FUN_ram_00015250`.
- **11 steps per full scan, 8 slots per step.** A full scan is 11 ADC triggers (step index 0..10);
  between steps the analog mux advances so a different set of keys is presented. `FUN_ram_00013314`
  reduces each step to 8 values: for each of 4 groups of 4 conversions it discards the first sample
  and averages the other three, for ADC0 and ADC1 interleaved, writing 8 values per step to
  `0x0008ee28 + step*8`. **11 × 8 = 88 slots** cover the 83 keys (5 spare).
- **8 kHz.** `FUN_ram_000002a0` increments `DAT_ram_00092004` once per completed scan and, at
  **8000**, snapshots the rate into `DAT_ram_00092020` and resets — i.e. exactly 8000 full scans per
  second. The USB side delivers this rate because every interrupt endpoint uses `bInterval = 1` at
  High Speed = one 125 µs microframe (see "USB").
- **Mux select.** No per-step GPIO writes exist in the code (no `GPIO0`/`FGPIO` register accesses),
  so the mux address lines are generated in hardware, synchronised to the ADC trigger — GPTMR1
  (`0xf0004000`) is re-synced each step in `FUN_ram_00015250` (set `0x4000`, clear `0x4000`,
  set `0x400`) and the PWM compare is retriggered in `FUN_ram_00015abe` (PWM0 `0xf03180f0` and PWM1
  `0xf031c0f0`: clear bit 7, set bit 3). The exact mux part and select pins need the schematic / a
  look inside the case (`open-questions.md`); functionally it is an 8-input × ~11-position scan.
- **Scan-slot → key-index tables** (both 88 bytes, in XIP):
  - `0x80040580` — used by the normal `BE FD` (1017) producer to order the 83 per-key words.
  - `0x80040528` — used by the baseline/auto-cal accumulator and the 1003 calibration-mode order.
  Values `0..82` are real keys (README row-by-row indices); values `83..87` are spare slots.

## RGB lighting

**85 addressable RGB LEDs, WS2812/SK6812 protocol, generated on SPI1 MOSI and streamed by HDMA.**
The per-LED colours and animations come from the QMK `rgb_matrix` subsystem (string
`rgb_matrix_init_drivers`, init `FUN_ram_00012ffa`); this driver is the physical layer.

- **Init** — `FUN_ram_00010f66`. Builds a 256-entry bit-expansion table at `0x00005090` (for each
  byte value, 8 × u16 SPI words where a 1 bit → `0xFFC0` and a 0 bit → `0xE000`), i.e. each LED data
  bit is sent as one 16-bit SPI word (long-vs-short high pulse = the WS2812 1/0 encoding). Then it
  resets SPI1 (`FUN_ram_00015b9c`), sets the SPI timing (`FUN_ram_000151f6`, source
  `DAT_ram_00007a74 = 12000000`), the frame format (`FUN_ram_00014e24`, `0x01010000`), and starts a
  DMA transfer of **0x7f9 = 2041 words** (= 4082 bytes). 85 LEDs × 3 bytes × 8 bits = 2040 data
  words + 1 trailing reset word = 2041. The SPI-timing-init failure path prints
  `SPI master timing init failed`.
- **Per-frame flush** — `FUN_ram_00013d78` (13 callers): rebuilds the bit-stream at `0x00006090`
  (`FUN_ram_000139fc`), then programs an HDMA channel (`0xf00c8000`, source `0x6090`, dest = SPI1
  TX FIFO `0xf007402c`, `0xff2` bytes) via DMAMUX (`0xf00c4000`) and starts it
  (`DAT_ram_f00c8040 |= 1`), polling `0xf00c8028` for completion.
- The config stores colours as 85 × (R,G,B) at `rgb` (`0x80080B00`); WS2812 chips take GRB on the
  wire, so `FUN_ram_000139fc` reorders. LED order (the chain snake and the space bar owning LEDs
  3/4/5) is documented from the vendor side in `panel/src/protocol/layout.ts`.

## USB

**USB 2.0 High-Speed with the HPM on-chip PHY** (`USB0` @ `0xf300c000`), CherryUSB device stack.

- The device descriptor at `0x80042164` reports `bcdUSB 0x0200`, `VID 0x36b0`, `PID 0x357e`,
  `bMaxPacketSize0 64`, 1 configuration. A **device-qualifier** descriptor is present at
  `0x8004222a` (`0a 06 00 02 ...`), which only a High-Speed-capable device provides — confirming HS
  enumeration.
- Config descriptor (`0x80042176`, `wTotalLength 116`, 4 interfaces, bus-powered 500 mA):

  | IF | Class / sub / proto | Endpoint(s) | mps | bInterval | Role (README `ifNN`) |
  |---|---|---|---|---|---|
  | 0 | HID / boot / keyboard | IN `0x81` | 64 | 1 | boot keyboard (if00) |
  | 1 | HID / 0 / 0 | IN `0x83` | 3 | 1 | consumer control (if01) |
  | 2 | HID / 0 / 0 (vendor) | IN `0x82`, OUT `0x03` | 1024 | 1 | **vendor raw HID, 0xFF60 (if02)** |
  | 3 | HID / boot / mouse | IN `0x84` | 3 | 1 | mouse (if03) |

- **How 8 kHz is reached.** Every interrupt endpoint has `bInterval = 1`. At High Speed that is
  `2^(1-1) = 1` microframe = **125 µs = 8000 Hz**. (At Full Speed the same `bInterval` would be
  1 ms = 1 kHz, so the HS PHY is what makes 8 kHz possible.)
- **Vendor interface (IF2).** Report descriptor begins at `0x800409f8`
  (`06 60 FF 09 61 ...` = Usage Page `0xFF60`, Usage `0x61`), 36 bytes, `bMaxPacketSize 1024` → the
  1024-byte reports. Commands arrive on OUT EP `0x03`; replies and the `BE FD` stream are sent on
  IN EP `0x82` via `FUN_ram_000150f0(0x82, &DAT_ram_00093a18, 0x400)` (a CherryUSB
  `usbd_ep_start_write`, **not** flash).

## Peripheral register map (addresses the firmware touches)

| Base | Peripheral | Used for |
|---|---|---|
| `0xf3080000` / `0xf3084000` | ADC0 / ADC1 | Hall sensor sampling |
| `0xf0318000` / `0xf031c000` | PWM0 / PWM1 | ADC trigger / mux timing |
| `0xf033c000` | TRGM0 | routes PWM→ADC trigger (`+0x180/0x184 = 0x30`) |
| `0xf0000000` / `0xf0004000` | GPTMR0 / GPTMR1 | scan timing / mux re-sync |
| `0xf0074000` | SPI1 | WS2812 LED bit-stream (MOSI) |
| `0xf00c8000` / `0xf00c4000` | HDMA / DMAMUX | SPI1 TX DMA for LEDs |
| `0xf300c000` | USB0 | High-Speed device |
| `0xf0040000` | UART0 | debug log / ROM UART-ISP pins |
| `0xf4040000` | IOC | pin mux |
| `0xf4000000` | SYSCTL | clocks/resets |
| `0xf40c0000` | PLLCTLV2 | PLLs |
| `0xf4100000` | PPOR | software reset (`BE FF` sets `+0x1c = 6000000`) |
| `0xe4000000` | PLIC | interrupts (bit 19 of `+0x2004` masked around flash writes) |
