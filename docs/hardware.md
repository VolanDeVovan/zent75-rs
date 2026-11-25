# Hardware

What our firmware has to drive. Reverse-engineered from the vendor images in `firmware/vendor/`
(0.0.18 is on the unit) and checked against the HPMicro SDK / HPM5361 SVD. `FUN_*`/`DAT_*` names are
Ghidra symbols of the 0.0.18 application (ILM @ `0x0`, DLM @ `0x80000`, XIP @ `0x80040000`); stage-0
addresses are XIP addresses of the boot image (`boot-recovery.md`).

## MCU

**HPMicro HPM5361, QFN48 (HPM5361IEG1).** The download loader embeds the vendor build path
`D:\ZentPro\zent75\…\soc\HPM5361\…`; the firmware uses both ADC0 and ADC1 (rules out HPM5301) and
exactly 8 analog pins (QFN48).

| | |
|---|---|
| Core | RV32IMAFDC (Andes core, custom CSRs `0x7CA`/`0x7CC` for cache control), up to 480 MHz, 16 KB I$ + 16 KB D$ |
| RAM | ILM 128 KB @ `0x00000000`, DLM 128 KB @ `0x00080000` (stock `sp` = `0x000A0000`) |
| Flash | 1 MB XIP NOR @ `0x80000000` via XPI0 (`0xF3000000`), accessed through the ROM API |
| ROM API | table @ `0x2001FF00` (`xpi_nor_driver_if` @ +0x14, `otp_driver_if` @ +0x0C) |
| USB | USB0 @ `0xF300C000`, built-in HS PHY |
| Rust | `hpm-hal` / `hpm-metapac` feature `hpm5361` |

## Flash map

| Region | Contents |
|---|---|
| `0x80000000..0x80002FFF` | XPI NOR config + HPM boot header (`0x80001000`, one image @ `0x80003000`) |
| `0x80003000..0x8001FFFF` | stage-0 "ZentBOOT" (TinyUF2 0.9.0 fork): boot decision + recovery key check |
| `0x80020000..0x8003FFFF` | download loader "Zent_DLD" (UF2 mass storage; target of `BE FC`) |
| `0x80040000..0x8006FFFF` | application. Word 0 = signature `0x0B4D5475`, entry `0x80040004`. Stock 0.0.18 ends at `0x80052900` |
| `0x80070000..` | config bank 1, board-global (top/bot, keytype, version, profile index, names) |
| `0x80080000..` | config bank 2, active profile (keymaps, actuation, RGB, DKS…) |

The first three regions must never be written — see `boot-recovery.md`. UF2 family id `0x0B4D5475`.
The stock app copies most of its code into ILM at boot through a SEGGER runtime-init table (LZ-packed).

## Peripherals used by the stock firmware

Register-level detail (values, init order, interrupt priorities): `re-peripherals.md`.

| Peripheral | Base | Use |
|---|---|---|
| ADC0 / ADC1 | `0xF3080000` / `0xF3084000` | hall sensors (ADC0 ch 1,3,5,7 · ADC1 ch 2,4,6,11) |
| PWM1 | `0xF031C000` | **analog-mux address** (outputs P4–P7 on PY00–PY03) |
| PWM0 + TRGM0 | `0xF0318000` / `0xF033C000` | ADC trigger: CH8REF → `ADC0_STRGI`/`ADC1_STRGI` (`TRGM0.trgocfg[32/33] = 0x30`) |
| GPTMR1 | `0xF0004000` | deferred "software interrupt": the ADC ISRs pulse it, its reload IRQ (lower priority) runs the per-step key processing |
| GPTMR0 | `0xF0000000` | free-running µs timebase (no IRQ) |
| SPI1 + HDMA + DMAMUX | `0xF0074000` / `0xF00C8000` / `0xF00C4000` | WS2812 RGB stream, MOSI = **PA29** |
| USB0 | `0xF300C000` | all HID interfaces |
| UART0 | `0xF0040000` | debug console (`[I/USB] …`) |
| IOC / PIOC | `0xF4040000` / `0xF4118000` | pin mux |
| PPOR | `0xF4100000` | reset / watchdog (`BE FF` writes `0xF410001C`) |
| PLIC | `0xE4000000` | interrupts |

## Hall-sensor scan

83 Hall switches → external analog multiplexers → 8 ADC inputs. One full scan = **11 mux steps ×
8 inputs = 88 slots** (83 keys + 5 spare), repeated at **8 kHz**.

**Sense inputs.** Pads **PB00, PB01, PB08–PB13** in analog mode (`FUN_ram_00015f08`). The stock app
splits them across the two ADCs, each running a 16-conversion hardware sequence per trigger
(channel list @ `0x800422B4`, 4 conversions per input, first one discarded, three averaged):

| ADC | Channels |
|---|---|
| ADC0 | 1, 3, 5, 7 |
| ADC1 | **2, 4, 6, 11** (the MMIO trace of 0.0.18 settles it; an older reading of the channel table said 0, 2, 4, 6) |

Within a step the 8 values are interleaved `[ADC0 ch1, ADC1 ch2, ADC0 ch3, ADC1 ch4, ADC0 ch5,
ADC1 ch6, ADC0 ch7, ADC1 ch11]` (`FUN_ram_00013314`), so **scan slot = step × 8 + position**.
IN1–IN5 = PB09–PB13, IN11 = PB08; IN6/IN7 are PB00/PB01 on this package (which is which is open).
Stage-0 reads all 8 inputs on ADC0 alone with channels `1,2,3,4,5,6,7,11`.

**Mux address.** Pads **PY00–PY03** = mux address bits 0–3. The app routes them to
**PWM1_P_4..P_7** (`PIOC 0xF4118E00..E18 = 3`, `IOC 0xF4040E00..E18 = 0x11`) and lets PWM1 generate
the sequence in hardware (`FUN_ram_0000EA98`): reload = 11 equal units, compare sets per channel
(`CHCFG` ch4: cmp 0–9, ch5: 10–14, ch6: 15–17, ch7: 18). Stage-0 drives the same pads as plain
PGPIO (`0xF411C1E4`/`0x1E8` set/clear). Both produce the address sequence

| Step | 0 | 1 | 2 | 3 | 4 | 5 | 6 | 7 | 8 | 9 | 10 |
|---|---|---|---|---|---|---|---|---|---|---|---|
| Address | 1 | 2 | 3 | 4 | 5 | 9 | 10 | 11 | 12 | 13 | 14 |

(mux inputs 0, 6, 7, 8 and 15 are unused). The mux IC itself is unidentified and does not matter for
firmware. Caveat: stage-0's slot 54 maps through the app's table to S, yet holding S alone did not
trigger it while A+S+D did (`boot-recovery.md`) — most likely the app files each step's sample one
step off from the address it was taken at. When we write our own scan, verify the slot→key table on
hardware instead of trusting either numbering.

**Slot ↔ key tables** (0.0.18, key index = README order, Esc = 0 … Right = 82; 83–87 = spare):

`0x80040528` scan slot → key:
```
slot  0- 7: 76 53 81 27 20 17 59 49     slot 48-55: 68 40 41 13  6  4 47 35
slot  8-15: 77 52 71 26 21 16 73 61     slot 56-63: 69 38 42 12  7  0 46 32
slot 16-23: 78 64 82 25 22 15 74 62     slot 64-71: 70 37 43 11  8  1 31 33
slot 24-31: 79 55 72 29 23 19 75 63     slot 72-79: 65 51 56 10  5  2 30 34
slot 32-39: 80 54 58 28 24 18 60 50     slot 80-87: 66 84 57  9 85 86 45 87
slot 40-47: 67 39 44 14 83  3 48 36
```
`0x80040580` key → scan slot (used to emit `BE FD` in key order):
```
key  0- 7: 61 69 77 45 53 76 52 60      key 48-55: 46  7 39 73  9  1 33 25
key  8-15: 68 83 75 67 59 51 43 21      key 56-63: 74 82 34  6 38 15 23 31
key 16-23: 13  5 37 29  4 12 20 28      key 64-71: 17 72 80 40 48 56 64 10
key 24-31: 36 19 11  3 35 27 78 70      key 72-79: 26 14 22 30  0  8 16 24
key 32-39: 63 71 79 55 47 65 57 41      key 80-82: 32  2 18
key 40-47: 49 50 58 66 42 86 62 54
```

**Levels.** Rest ≈ 30000 counts, bottom-out ≈ 18000 (16-bit result; ≈ 7.5 µm/count over ~3.4 mm).

**Timing.** PWM period 20 000 AHB ticks (160 MHz) = 125 µs; mux step every 1818 ticks
(11.36 µs); ADC trigger 320 ticks (2 µs) after each step boundary. ADC clock = AHB / 4 = 40 MHz
(`conv_cfg1 = 0x153`, 16-bit), so one 16-conversion sequence takes ≈ 10 µs — the last
conversions of a step end close to the next mux switch (worth keeping in mind for the
one-step-offset question below). ADC DMA lands in DLM `0x00092260` (ADC0) / `0x00092160` (ADC1),
16-word circular buffers; the per-ADC `seq_cvc` interrupts (`FUN_ram_000002A0` /
`FUN_ram_000003A6`, PLIC prio 5) count steps 0…10, mark the step ready and pulse GPTMR1; the
GPTMR1 handler (`FUN_ram_0000057A`, prio 2) runs the per-step processing
`FUN_ram_00007A88(step)`. 8000 full scans/s.

Our firmware (`firmware/app/src/scan.rs`) programs the same registers, but works out the step
from the PWM0 counter at interrupt time instead of counting, and runs `Keyboard::scan` once per
complete 88-sample frame. The step ↔ sample association is one constant there (`STEP_OFFSET`).

## RGB

**85 WS2812-class LEDs on SPI1 MOSI = PA29** (`IOC PA29 = 5`; no SCLK/CS pads muxed), 12 MHz SCLK
(24 MHz source ÷ 2 — stock never sets the SPI1 clock, our firmware sets 24 MHz explicitly),
16-bit frames, one SPI word per LED data bit (`1` → `0xFFC0`, `0` → `0xE000`, 1.333 µs per bit),
GRB byte order, streamed by HDMA ch0 (DMAMUX request `0x2B`) from a 4082-byte buffer
(85 × 24 × 2 + 2) (`FUN_ram_00010F66` init, `FUN_ram_00013D78` refresh). The chain snakes from the
bottom-left upward; Space owns three LEDs. Key → LED table: `KEY_TO_LED` in
`panel/src/protocol/layout.ts`. No LED power-enable GPIO exists.

## USB

Device `36B0:357E`, "Wob Lab" / "ZENT  75 HE ", CherryUSB, High Speed. All interrupt endpoints use
`bInterval = 1` → 125 µs microframes = **8 kHz**.

| If | Class | Endpoints | Role |
|---|---|---|---|
| 0 | HID boot keyboard | `0x81` IN, 64 B | keyboard |
| 1 | HID | `0x83` IN, 3 B | consumer (24-usage bitmap; no System Control collection) |
| 2 | HID vendor, usage page `0xFF60`, usage `0x61` | `0x82` IN + `0x03` OUT, 1024 B | configurator protocol (`protocol.md`) |
| 3 | HID mouse | `0x84` IN, 3 B | mouse-button keycodes 251–255 |

Descriptors: device @ `0x80042164`, configuration @ `0x80042176`; byte dumps and the four report
descriptors in `re-peripherals.md` §5. The USB serial string is the fixed `"20240728"`, not the
chip UID.

## Serial number

128-bit chip UID from OTP words `0x58..0x5B` via the ROM OTP driver (`FUN_ram_0001550E`), returned by
`BE 1D`.
