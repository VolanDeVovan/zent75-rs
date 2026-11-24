# Hardware map — HOVEY WORKS ZENT75-HE

Reverse-engineered from the vendor firmware (`firmware/vendor/0.0.18.uf2`, the build on the unit, and
`0.0.9.uf2`), the HPMicro SDK in `re/cache/hpm_sdk/`, and the HPM5300 datasheet
(`HPM5300DSV012`, Rev 0.12). Function/data addresses are Ghidra symbols in `re/out/0.0.18/decomp.c`
(reconstructed RAM image: ILM @ `0x0`, DLM @ `0x80000`, XIP app @ `0x80040000`). Print any function
with `re/fn.sh 0.0.18 <hexaddr>`.

## 1. MCU

**HPMicro HPM5300-series, QFN48 package — an HPM5361-class part (HPM53x1IEG1, almost certainly
HPM5361IEG1). HPM5301 is ruled out.**

| Property | Value | Evidence |
|---|---|---|
| Core | 32-bit RISC-V, **RV32-IMAFDC(BP)**, up to 480 MHz, 3.57 CoreMark/MHz | datasheet §1.2; compiler used C/F/D/A extensions throughout decomp |
| Caches | 16 KB L1 I-cache + 16 KB L1 D-cache | `hpm_soc_feature.h` `HPM_L1C_CACHE_SIZE` |
| ILM | 128 KB @ `0x00000000` | `toolchains/gcc/flash_uf2.ld`; SEGGER init sets `sp=0xA0000` (`re/segger_init.py`) |
| DLM | 128 KB @ `0x00080000` | same |
| On-chip SRAM | 288 KB total (ILM+DLM+shared) | datasheet §1.2 |
| Flash | 1 MB internal XIP NOR @ `0x80000000` | datasheet; UF2 targets `0x8004xxxx` (`re/uf2.py`) |
| BOOT ROM | 128 KB, ROM API table @ `0x2001FF00` | `hpm_romapi.h` `ROM_API_TABLE_ROOT` |
| OTP | 4096-bit; holds the 128-bit UID | serial read `FUN_ram_0001550e` |
| USB | 1× USB 2.0 OTG with **built-in HS PHY** | datasheet; `USB0` @ `0xf300c000` used |

### Why HPM5361-class QFN48, not HPM5301

* **Two ADCs are used.** The firmware drives both `ADC0` (`0xf3080000`) and `ADC1` (`0xf3084000`)
  — see `FUN_ram_0001023c` (ADC init, decomp lines ~6668–6724) and `FUN_ram_00013314` (sample
  reader). The datasheet ordering table (§6.2, table 44) gives HPM5301 **one** ADC (footnote: "only
  ADC0 available"); HPM5361/5331/5321 have **2× 16-bit** ADC. Two ADCs ⇒ not HPM5301.
* **QFN48 package.** The scan samples exactly **8 analog channels** — ADC-A reads channels 1,3,5,7
  and ADC-B reads channels 0,2,4,6 (channel list at `0x800422b4`, loaded in `FUN_ram_0001023c`).
  Datasheet table 45 ("package pin-function differences"): QFN48 exposes **8 analog pins**, LQFP64
  exposes 10, LQFP100 exposes 16. 8 analog pins ⇒ QFN48 (`HPM53x1IEG1`).
* **Exact functional SKU is not decidable from firmware.** HPM5361 (full), HPM5331 (motion-control)
  and HPM5321 (comms) share the same die and peripheral map; the keyboard only touches peripherals
  common to all (ADC/USB/SPI/PWM/GPTMR/timers), so it cannot distinguish them. The HPMicro SDK the
  firmware was built against ships the `HPM5361` SVD (and `HPM53M1`, a newer variant absent from the
  DSV012 datasheet, with an identical peripheral map); the peripheral base addresses below match it
  exactly. The definitive marking (and package) needs the physical chip — see `open-questions.md`.

### Package part-number decode (datasheet figure 23)

`HPM53` **F** `1` **I** **PP** `1` — F = function (6 full / 3 motion / 2 comms / 0 basic),
I = industrial −40…105 °C, PP = package (**CB** = 14×14 LQFP100, **CF** = 10×10 LQFP64,
**EG** = 6×6 QFN48). Our board is an `…EG1` (QFN48). GPIO counts: 56 / 36 / **29** for
LQFP100 / LQFP64 / QFN48.

### Memory and flash layout

ILM and DLM are both local RAM; the stock firmware copies most code from XIP flash into ILM at boot
through a SEGGER runtime-init table (`re/segger_init.py`; `sp = 0x000A0000` = top of DLM). Flash map
(verified against `re/uf2.py` and the SDK linker scripts):

| Region | Address | Notes |
|---|---|---|
| Boot / `nor_cfg_option` | `0x80000000` | XPI NOR flash config |
| **TinyUF2 bootloader** | `0x80020000` | SDK reserves `0x20000`; the vendor reserves `0x40000` (see `bootloader.md`) |
| **Application** | `0x80040000` | word0 = `0x0b4d5475` boot signature; entry `0x80040004` |
| Application end | 0.0.18: `0x80052900`; 0.0.9: `0x80052200` | from `re/uf2.py` |
| **Config bank 1** (board-global) | `0x80070000` | top/bot/keytype/version/profile/names; shadow ILM `0x7084` |
| **Config bank 2** (active profile) | `0x80080000` | keymaps, actuation, RGB, DKS…; shadow ILM `0x658` |

The vendor `.uf2` writes only `0x80040000..0x80052900`, so the two config banks survive a reflash
(verified byte-identical). UF2 family id = `0x0b4d5475` (vendor-private). Both banks are mirrored into
RAM shadows addressed by `BE 03`/`BE 04` — see `protocol.md`.

## 2. Peripherals in use

Base addresses resolved against `re/cache/hpm_sdk/soc/HPM5300/HPM5361/HPM5361_svd.xml`. All appear
as `DAT_ram_<base>` in the decomp (the registers are outside the reconstructed RAM image).

| Peripheral | Base | Used for |
|---|---|---|
| `ADC0` | `0xf3080000` | hall-sensor readout (odd mux channels) |
| `ADC1` | `0xf3084000` | hall-sensor readout (even mux channels) |
| `PWM0` | `0xf0318000` | ADC conversion trigger; scan pacing (`FUN_ram_00012f24`, `FUN_ram_00015abe`) |
| `PWM1` | `0xf031c000` | scan pacing (`FUN_ram_00015abe`) |
| `TRGM0` | `0xf033c000` | routes PWM → ADC trigger; `TRGM0+0x180/0x184 = 0x30` in `FUN_ram_0001023c` |
| `GPTMR0` | `0xf0000000` | timing |
| `GPTMR1` | `0xf0004000` | per-step scan sync/advance (`FUN_ram_00015250`, `FUN_ram_0000057a`) |
| `SPI1` | `0xf0074000` | RGB LED data (WS2812-over-SPI) |
| `HDMA` | `0xf00c8000` | DMA for the RGB SPI stream (`FUN_ram_00013d78`) |
| `DMAMUX` | `0xf00c4000` | DMA request routing |
| `UART0` | `0xf0040000` | debug console (`[I/USB]`…); also the ROM ISP UART |
| `USB0` | `0xf300c000` | all HID interfaces |
| `IOC` | `0xf4040000` | pin mux |
| `PIOC` | `0xf4118000` | power-domain pin control (`0xf4118e00/e08/e10/e18 = 3`) — mux-select candidate |
| `PPOR` | `0xf4100000` | reset controller; `BE FF` writes `0xf410001c` |
| `SYSCTL`/`PLLCTLV2` | `0xf4000000`/`0xf40c0000` | clocks |
| `PLIC` | `0xe4000000` | interrupts; `0xe4002004` bit 19 gated around flash ops |

## 3. Hall-sensor readout (the scan)

The 83 Hall switches are read as analog voltages through **analog multiplexers** into the two ADCs.
There are only 8 analog pins on the QFN48, so the keys are time-multiplexed.

**Hardware pipeline** (`FUN_ram_0001023c`):
1. `PWM0` (+ `PWM1`) generate a periodic trigger; `TRGM0` routes it to both ADCs
   (`TRGM0+0x180/0x184 = 0x30`). The ADCs run in a repeating hardware-triggered sequence — no CPU in
   the sample loop.
2. Each ADC runs a **16-conversion sequence** per trigger (`bStack_24 = 0x10`). The channel list is
   at `0x800422b4`: **ADC-A** = `1,1,1,1, 3,3,3,3, 5,5,5,5, 7,7,7,7`; **ADC-B** =
   `0,0,0,0, 2,2,2,2, 4,4,4,4, 6,6,6,6`. So each ADC samples **4 physical analog pins, 4× each**
   (one settling sample discarded + three averaged). A 5th channel (`11`) is sampled separately as a
   reference/supply monitor (reported in the `BE FD` tail, see `protocol.md`).
   * **The 8 analog-input pads are confirmed from the binary.** `FUN_ram_00015f08` (called first thing
     in `FUN_ram_0001023c`) sets exactly 8 `IOC` pads to **analog mode** by writing `0x100`
     (`IOC_PAD_FUNC_CTL_ANALOG`, bit 8) to `HPM_IOC` (`0xf4040000`) `PAD[n].FUNC_CTL` at offsets
     `0x100,0x108,0x140,0x148,0x150,0x158,0x160,0x168` → pad indices `32,33,40–45` = **PB00, PB01,
     PB08, PB09, PB10, PB11, PB12, PB13** (HPM pad numbering PA=0–31, PB=32–63; verified against the
     HPM5361 `hpm_ioc_regs.h`/`hpm_iomux.h`). These are the 8 ADC sense inputs (channels 0–7) fed by
     the external analog mux(es); they are the "8 analog pins on QFN48" from datasheet table 45. (Note
     PB00/PB01 double as UART0 and PA04–PA08 as JTAG in ISP/boot stages — no conflict, different boot
     stage; see `bootloader.md`.)
3. Results DMA into `0x80092260` (ADC-A) and `0x80092160` (ADC-B), 16 × u32 = `0x40` bytes per ADC
   per trigger.

**Per-trigger interrupt** (`FUN_ram_000002a0` for ADC-A, `FUN_ram_000003a6` for ADC-B):
* copies the `0x40`-byte DMA block into a growing scratch buffer (`0x8008d408` ADC-A, `0x8008d148`
  ADC-B), advancing `0x40` each time;
* a step counter `DAT_ram_000920ec` runs 0…**10** (11 steps); after both ADCs finish a step, sets a
  per-step "valid" flag `(&DAT_ram_00091d98)[step]` and calls `FUN_ram_00015250` (advances the scan /
  the analog-mux position via `GPTMR1`);
* after step 10 (one full scan) it increments the 1-second scan counter `DAT_ram_00092004`; when it
  reaches **8000** it latches the rate into `DAT_ram_00092020` and resets — **8000 scans/s = 8 kHz**.

So one full scan = **11 mux positions × 2 ADCs × 4 pins = 88 slots** (83 keys + 5 spare). The analog
mux select advances one position per step. **Which pins drive the mux select is not cleanly in the
binary** — the advance is done inside `FUN_ram_00015250` (a `GPTMR1` reload: set `0x4000`, clear,
set `0x400`) with no explicit per-step GPIO write visible; the four `PIOC` pads at
`0xf4118e00/e08/e10/e18` (set to `3` once at init) are the most likely select lines. Pinning this
down needs a logic analyzer / schematic — see `open-questions.md`.

**Slot → key mapping.** The 88 raw slots are averaged by `FUN_ram_00013314` into `0x8008ee28`
(step-ordered), then remapped to the 83 README key indices (Esc=0 … Right=82) by the permutation
tables at `0x80040528` (88 bytes, scan-slot → key) and `0x80040580` (88 bytes, used to emit the
`BE FD` stream in key order). Values 83…87 in those tables are the 5 spare slots. Example table-0
head: `76,53,81,27,20,17,59,49,…` (slot 0 carries key 76 = Space). The per-key working arrays
(baseline, span, travel; see `algorithms.md`) are indexed by **scan slot** throughout the tick and
only remapped to key index when a `BE FD` frame or a HID report is built.

Resting ADC ≈ 30000 counts, bottom-out ≈ 18000 (≈1 count ≈ 7.5 µm over a ~3.4 mm switch).

## 4. RGB lighting

**85 addressable RGB LEDs (WS2812 / SK6812 family) on `SPI1` via DMA.** The firmware uses the QMK
`rgb_matrix` subsystem (string "rgb_matrix_init_drivers rgb_matrix_config.mode = 0. Write default
values to EEPROM." @ `0x800403cc`, logged by `FUN_ram_00012ffa`).

**Driver backend** (`FUN_ram_00010f66` init, `FUN_ram_00013d78` refresh):
* `SPI1` configured as master, **12 MHz** (`DAT_ram_00007a74 = 12000000`), **16-bit** data frames
  (`DAT_ram_00091fa8` format word `0x01010000`). `FUN_ram_000151f6` is the SPI timing init — the
  "SPI master timing init failed" string (`0x80041c28`) guards it.
* Each LED **data bit** is encoded as one 16-bit SPI word: a `1` bit → `0xffc0` (long high pulse),
  a `0` bit → a short-high pattern. At 12 MHz a 16-bit word ≈ 1.33 µs, matching the WS2812 1.25 µs
  bit cell.
* Encode buffer at `0x80006090`, size `0xff2` = 4082 bytes = **85 LEDs × 24 bits × 2 bytes + 2**
  (the `+2` latch gap). The DMA transfer is `0x7f9` = 2041 16-bit words (= 4082 bytes).
* `FUN_ram_00013d78` arms `HDMA` (`0xf00c8000`) to stream the buffer to the `SPI1` TX FIFO
  (`0xf007402c`) and waits for completion.

**LED order / chain.** The chain snakes from the bottom-left upward; the space bar (key 76) owns
LEDs 3/4/5. The full key→LED table is `KEY_TO_LED` in `panel/src/protocol/layout.ts`. The RGB config
RAM regions are `rgb` (85×3 @ `0x80080B00`), `rgb_solid_enable` (85 @ `0x80080C08`) and `rgb_config`
(5 @ `0x80080C60`) — see `protocol.md`.

**The SPI1 MOSI pin that drives the LED data line is not resolved** (needs the `IOC` mux dump /
schematic) — `open-questions.md`.

## 5. USB

Device descriptor @ `0x80042164`: `bcdUSB = 0x0200`, `bMaxPacketSize0 = 64`, **VID `0x36B0`,
PID `0x357E`**, 1 configuration. String descriptors: manufacturer **"Wob Lab"**, product
**"ZENT  75 HE "**. The HPM5300 built-in HS PHY is used.

Configuration descriptor @ `0x80042176` (`wTotalLength = 116`, 4 interfaces, 500 mA):

| If | Class | Report-desc | Endpoints | Role |
|---|---|---|---|---|
| 0 | HID boot keyboard (3/1/1) | 61 B @ `0x800420d4` | `0x81` INTR IN, mps 64, bInterval 1 | main keyboard (6KRO/NKRO) |
| 1 | HID (3/0/0) | 80 B @ `0x80040a1c` | `0x83` INTR IN, mps 3, bInterval 1 | consumer control + system |
| 2 | HID vendor (3/0/0) | 36 B @ `0x800409f8` | `0x82` INTR IN + `0x03` INTR OUT, **mps 1024**, bInterval 1 | **vendor raw HID** (configurator) |
| 3 | HID mouse (3/1/2) | 50 B @ `0x80041c68` | `0x84` INTR IN, mps 3, bInterval 1 | mouse buttons (keycodes 251–255) |

* **Vendor interface (if2)** report descriptor begins `06 60 FF 09 61 A1 01 …` = Usage Page `0xFF60`,
  Usage `0x61`, with 1024-byte IN (`81 02`) and OUT (`91 02`) reports — matching the README
  (`0xFF60`/`0x61`, 1024-byte reports, report id 0). The OUT endpoint `0x03` receives the 1024-byte
  command frames into `0x80093618`; the IN endpoint `0x82` ships replies and the `BE FD` stream
  (`FUN_ram_000150f0(0x82, …)`, a CherryUSB `usbd_ep_start_write`).
* **8 kHz polling.** Every interrupt endpoint has `bInterval = 1`. At **High Speed**, `bInterval = 1`
  means `2^(1-1) = 1` microframe = 125 µs = **8 kHz**. (At Full Speed the same value would be 1 ms =
  1 kHz.) So the advertised 8 kHz requires the device to enumerate at High Speed via the built-in HS
  PHY. The mouse endpoint `0x84` is **new in 0.0.18**; 0.0.9 has only `0x80`–`0x83` (see
  `stable-vs-latest.md`).
* USB stack: **CherryUSB** (strings "cherry usb hid_keyboard device sample.", "[I/USB]",
  "Unhandled HID Class bRequest").

## 6. Serial number

`FUN_ram_0001550e` reads the **128-bit chip UID** from OTP (indices `0x58`…`0x5b`) through the BOOT
ROM API — `ROM_API_TABLE_ROOT->otp_driver_if->read_from_shadow` (`_DAT_ram_2001ff0c`, `hpm_romapi.h`
line 244/300) — into `0x80091f44` (16 bytes). `BE 1D` returns these 16 bytes (e.g. the unit's
`0123456789abcdef0123456789abcdef`, `backups/1795494802/dump.json`).
