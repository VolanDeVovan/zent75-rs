# Hardware map — ZENT75-HE (Track A)

All evidence is from the Ghidra decompilation of the stock firmware (`re/out/0.0.18/decomp.c`,
functions printed with `re/fn.sh 0.0.18 <addr>`), the reconstructed RAM images
(`re/out/0.0.18_{ilm,dlm}.bin` and the XIP image `re/out/0.0.18_80040000.bin`), the HPMicro SDK in
`re/cache/hpm_sdk/`, and the HPM5300 datasheet. The device runs firmware **0.0.18**; 0.0.9 is
structurally identical where noted.

## 1. MCU

**HPMicro HPM5361 (HPM5300-family RISC-V, 2-ADC die), in a QFN48 package — part `HPM5361ICB1` or a
pin-compatible `HPM53x1ICB1`.**

Evidence:

- **RISC-V RV32IMAFDC.** Startup sets `gp` and uses `fence.i`, `csr` 0x7ca/0x7cc (Andes custom
  cache-control CSRs), hardware float/double helpers (`FUN_ram_80042830` etc. are soft/hard FP
  routines used by the switch-curve maths). The Andes CSRs (`mcache_ctl` 0x7ca, `mcctlcommand`
  0x7cc) are specific to the Andes D25F/D27 cores HPMicro uses in the HPM5300.
- **Peripheral base addresses** dereferenced in the code match the HPM5300 SVD
  (`re/cache/hpm_sdk/soc/HPM5300/HPM5361/HPM5361_svd.xml`) exactly:

  | Base in code | Peripheral | Used for |
  |---|---|---|
  | `0xf3080000` | **ADC0** | hall-sensor readout |
  | `0xf3084000` | **ADC1** | hall-sensor readout |
  | `0xf0318000` | PWM0 | ADC conversion trigger (via TRGM) |
  | `0xf031c000` | PWM1 | ADC trigger (second group) |
  | `0xf033c000` | TRGM0 | routes PWM → ADC trigger (`+0x180/0x184 = 0x30`) |
  | `0xf0000000` / `0xf0004000` | GPTMR0 / GPTMR1 | scan time-base / mux step |
  | `0xf0074000` | **SPI1** | WS2812 RGB data |
  | `0xf300c000` | USB0 | the HS USB device |
  | `0xf0040000` | UART0 | debug console / ROM ISP |
  | `0xf4040000` | IOC | pin mux |
  | `0xf4000000` | SYSCTL | clocks/resets |
  | `0xf410001c` | watchdog | `BE FF` reboot timer |
  | `0x2001FF00` | ROM API table root | flash read/erase/program (see `bootloader.md`) |

- **Two ADC instances are used** (`FUN_ram_0001023c` initialises both `0xf3080000` and
  `0xf3084000`, and both DMA handlers run). In the SDK SoC headers only the **single-ADC HPM5301**
  has one ADC; the **HPM5361** and the near-identical **HPM53M1** expose `HPM_ADC0_BASE`+`HPM_ADC1_BASE`
  (`re/cache/hpm_sdk/soc/HPM5300/HPM53{61,M1}/hpm_soc_ip.h:373-379`). **This rules out the HPM5301.**
- HPM5361 and HPM53M1 are indistinguishable from the firmware alone (their `hpm_soc_ip_feature.h`
  differ only by a copyright line). HPM5361 is the mainstream orderable 2-ADC part; HPM53M1 is a
  later variant with the same peripheral map, and it is **not listed in the HPM5300 series datasheet**
  (HPM5300DSV012, which enumerates only HPM5361/5331/5321/5301), so the named part is in the HPM536x
  family. The datasheet (§1) gives the family as 2 × 16-bit ADC/2MSPS with **16 analog input pins
  total**, USB2.0 OTG with integrated HS-PHY. Only 8 of those analog pins are bonded out on QFN48;
  the scan below uses exactly 8 sense lines, which points at **QFN48 specifically** (LQFP64/LQFP100
  expose more). The ordering-code package suffix confirms the convention `…B1` = QFN48, `…F1` =
  LQFP64, `…G1` = LQFP100 (HPM5361 ships as `HPM5361ICB1`/`ICF1`/`IEG1`), so the best single
  identification is **`HPM5361ICB1` (HPM5361, QFN48)** — or a pin-compatible `HPM53x1ICB1` sibling
  that differs only in flash/security options the firmware does not exercise. The die silkscreen is
  the only way to be 100 % certain (see `open-questions.md`).
- **HS USB with built-in PHY**: USB0 at `0xf300c000`, and the descriptors below use a 1024-byte
  interrupt endpoint which is only legal at High Speed.

ADC capability (SDK `hpm_soc_feature.h`): ADC16 IP v3, 16-element sequence, 16-bit result, 2 MSPS
class. The firmware runs both ADCs as hardware-triggered 16-conversion sequences into DMA (below).

## 2. Memory and flash layout

XIP NOR flash based at `0x80000000`; the on-chip local memories are **ILM 128 KB @ `0x00000000`**
and **DLM 128 KB @ `0x00080000`** (SDK `flash_uf2.ld`: `ILM LENGTH=128K`, `DLM ORIGIN=0x00080300`;
`re/segger_init.py` sets `sp=0x000A0000` = top of DLM). The stock firmware copies most code from XIP
into ILM at boot through a SEGGER runtime-init table and then runs from RAM.

Flash map (verified against the UF2 images with `re/uf2.py` and against the SDK linker scripts):

| Region | Address | Notes |
|---|---|---|
| Boot / `nor_cfg_option` | `0x80000000` | XPI NOR flash config |
| **TinyUF2 bootloader** | `0x80020000` | the SDK reserves `0x20000`; the vendor reserves `0x40000` |
| **Application** | `0x80040000` | `BE FC` jumps here-minus-0x20000; word0 = `0x0b4d5475` signature; entry `0x80040004` |
| Application end | 0.0.18: `0x80052900`; 0.0.9: `0x80052200` | from `re/uf2.py` |
| **Config bank 1** (board-global) | `0x80070000` | top/bot/keytype/version/profile/names |
| **Config bank 2** (active profile) | `0x80080000` | keymaps, actuation, RGB, DKS… |

The vendor `.uf2` only writes `0x80040000..0x80052900`, so the two config banks survive a reflash
(verified byte-identical). The UF2 family id is `0x0b4d5475` (vendor-private). Both banks are
mirrored into RAM shadows that the `BE 03`/`BE 04` commands address — see `protocol.md`.

## 3. Hall-sensor readout (the scan)

The 83 keys are magnetic (hall) switches read through **analog multiplexers** into the two ADCs.

### Hardware trigger and DMA

`FUN_ram_0001023c` (ADC init) does: reset both ADCs (`FUN_ram_00012894`), load per-ADC channel
lists from a table at `0x800422b4`, configure each as a **16-conversion sequence** (`bStack_24=0x10`)
with DMA to `0x00092260` (ADC0) and `0x00092160` (ADC1), set the bus-mode/convert-enable bit
(`0xf3081114|=0x800000`, `0xf3085114|=0x800000`), and arm PWM0 + TRGM0 as the hardware trigger
(`0xf033c180=0x30`, `0xf033c184=0x30`). So both ADCs fire their 16-conversion sequence on every PWM
trigger, straight into DMA, with no CPU involvement per conversion.

### Per-trigger DMA completion

`FUN_ram_000002a0` (ADC0) and `FUN_ram_000003a6` (ADC1) are the DMA-complete handlers:

- Each copies its 0x40-byte (16 × u32) result block to a growing staging buffer
  (`0x0008d408` for ADC0, `0x0008d148` for ADC1), advancing by 0x40 each time.
- A step counter `DAT_ram_000920ec` runs **0..10 (11 steps)**; `DAT_ram_000920b8` ORs in bit0 (ADC0)
  / bit1 (ADC1), and when both are in (`==3`) it marks that step valid in `(&DAT_ram_00091d98)[step]`
  and advances the mux via `FUN_ram_00015250`.
- At step 11 the staging pointers reset. A free-running counter `DAT_ram_00092004` counts scans and
  resets every **8000** — i.e. the full scan runs at **8 kHz** (and 8000 is streamed in the `BE FD`
  tail; see `protocol.md`).

### Scan geometry → key mapping

`FUN_ram_00013314(step)` reduces one step's raw data: for each ADC it walks 4 groups of 4
conversions (stride 0x10), **discards the first sample of each group and averages the other three**,
producing **4 values per ADC per step = 8 per step**. Over 11 steps that is **88 slots** for 83 keys
(`0x0008ee28`, 88 × u32).

So the electrical picture is **8 analog sense lines (4 into ADC0, 4 into ADC1), each the common pin
of an analog multiplexer whose select is stepped through 11 positions**, giving 8 × 11 = 88 sense
points. This matches the README note that the Rainy75-HE sibling uses Nexperia (74HC4067-class 16:1)
muxes — here ~11 of the 16 positions are populated. The mux step is driven by `FUN_ram_00015250`,
which pulses GPTMR1 (`0xf0004000 |=0x4000; &=~0x4000; |=0x400`) once per completed step; GPTMR1's
compare outputs are the mux-select lines. **No steady-state GPIO writes to the select lines appear
in the code**, so the select is generated by the GPTMR1 output pins, not bit-banged — the exact pins
need the IOMUX cross-reference / schematic (see `open-questions.md`).

### Scan-slot ↔ key-index tables

Two 88-byte permutation tables in the XIP image encode the mapping (dumped in
`re/scan_tables.txt`):

- `0x80040528[scan_slot] = key_index` (scan order → the README's row-by-row key index; values
  0..82 are real keys, 83..87 are unused slots)
- `0x80040580[key_index] = scan_slot` (the inverse; used by the `BE FD` producer to emit per-key
  data in key-index order)

The per-key working arrays (baseline, span, travel; see `algorithms.md`) are indexed by **scan slot**
throughout the tick and only remapped to key index when a `BE FD` frame or a HID report is built.

Resting ADC ≈ 30000 counts, bottom-out ≈ 18000 (one count ≈ 7.5 µm over the ~3.4 mm switch).

## 4. RGB lighting

**85 WS2812B-class addressable LEDs on a single data line driven by SPI1 (`0xf0074000`) MOSI at
12 MHz**, DMA-fed, on top of the QMK `rgb_matrix` subsystem.

Evidence (`FUN_ram_00010f66`, the LED init):

- Builds a 256-entry expansion table at `0x5090` (4096 bytes): each colour byte → 8 × u16 SPI
  patterns, **`0xE000` for a `0` bit, `0xFFC0` for a `1` bit** — the classic WS2812 "one SPI word per
  data bit" encoding. At the 12 MHz SPI clock (`DAT_ram_00007a74=12000000`) each 16-bit word is
  ~1.33 µs, matching WS2812 bit timing.
- The LED frame buffer is at `0x6090` (0xff2 = 4082 bytes ≈ 85 LEDs × 24 bits × 2 bytes).
- Sets up SPI1 master timing (`FUN_ram_00015b9c`/`FUN_ram_000151f6`; on failure prints
  `"SPI master timing init failed"`), then a DMA transfer (`FUN_ram_000140c0(... ,0x7f9,0)`; on
  failure prints `"spi setup dma transfer failed"` and hangs), and enables a self-reloading DMA
  descriptor (`0xf00c4000 = 0x8000002b`) for continuous refresh.
- The data pin is an IOC pad set to function 5 (`0xf40400e8 = 5`); exact pin needs the IOMUX map.

The QMK layer is present: the string `rgb_matrix_init_drivers` and
`"rgb_matrix_config.mode = 0. Write default values to EEPROM."` appear in the image. Effects are
selected by `rgb_config` and the per-LED colour / static-vs-dynamic state live in config bank 2
(`rgb` @ `0x80080B00`, `rgb_solid_enable` @ `0x80080C08`); see `protocol.md`. The key→LED order
(space bar = LEDs 3,4,5) is in `panel/src/protocol/layout.ts`.

## 5. USB

The device descriptors are at `0x80042164` (device) and `0x80042176` (configuration), decoded from
the XIP image:

- **Device**: bcdUSB 2.00, class per-interface, bMaxPacketSize0 64, **idVendor `0x36b0`,
  idProduct `0x357e`**, bcdDevice 0x0002, strings iMfr/iProduct/iSerial = 1/2/3 (`"Wob Lab"`,
  `"ZENT  75 HE"` as UTF-16 at `0x800421f0`). CherryUSB stack (`"cherry usb hid_keyboard device
  sample."`, `"[I/USB]"`).
- **Configuration** (wTotalLength 116, bmAttributes 0x80, bMaxPower 250 → 500 mA), **4 interfaces**,
  all endpoints `bInterval = 1`:

  | If | Class / proto | Report desc | Endpoint | wMaxPacketSize |
  |---|---|---|---|---|
  | 0 | HID boot keyboard (3/1/1) | 61 B | `0x81` IN intr | 64 |
  | 1 | HID consumer (3/0/0) | 80 B | `0x83` IN intr | 3 |
  | 2 | **HID vendor raw** (3/0/0) | 36 B | `0x82` IN + `0x03` OUT intr | **1024** |
  | 3 | HID boot mouse (3/1/2) | 50 B | `0x84` IN intr | 3 |

- The vendor interface's report descriptor is a vendor-defined collection, **Usage Page `0xFF60`,
  Usage `0x61`** (`0x06 0x60 0xFF` at `0x800409f8`, `0x61` at `0x800421fa`) — the 1024-byte raw-HID
  channel the configurator uses. `FUN_ram_000150f0(0x82, …, 0x400)` writes replies on EP `0x82`.

**How 8 kHz is reached.** A **1024-byte interrupt endpoint is illegal at Full Speed** (FS caps
interrupt at 64 bytes), so the device *must* enumerate at **High Speed** (built-in HS PHY). At HS the
1 ms frame is split into 8 × 125 µs microframes, and `bInterval = 1` means "service every microframe"
= **8000 Hz**. The firmware's own 8 kHz scan rate (above) is matched to this poll rate. All four HID
interfaces poll at bInterval 1 (8 kHz worth of microframes); the boot keyboard/mouse/consumer carry
little data, the vendor channel carries the 1024-byte config and `BE FD` reports.
