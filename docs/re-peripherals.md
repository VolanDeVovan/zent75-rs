# Stock firmware 0.0.18 — peripheral configuration

Driver-level reference for re-implementing the ZENT75-HE app on the HPM5361. Everything here was
read from the stock 0.0.18 app: the MMIO trace of its startup under emulation (`re-work/v18_mmio.log`,
decoded against hpm-metapac `hpm5361`), single functions run from the post-init snapshot, and static
disassembly. Addresses `0x0xxxx` are ILM (unpacked code), `0x8004xxxx` are XIP. `gp = 0x00091C2C`.
Register names are hpm-metapac names. "PLIC IRQ n" uses the HPM5361 numbering (metapac `INTERRUPTS`).
Where only the emulator saw a value (reads return 0 there) this is said explicitly.

Corrections to `hardware.md`: **ADC1 converts channels 2, 4, 6, 11** (not 0, 2, 4, 6); the
per-step "processing" timer is GPTMR1 but **GPTMR0** is a separate µs timebase; the **WS2812 data pin
is PA29** (SPI1_MOSI).

## 1. Clocks

Startup is the HPM SDK `board_init_clock()` pattern (`0x0000D1D0`, `0x0000D590`, `0x00015030`).

| Register | Value | Meaning |
|---|---|---|
| `PLLCTL.xtal` | `0x00046500` (bits 19:0) | XTAL ramp time |
| `PLLCTL.pll[0].mfi/mfn` | `0x28` / `0` | PLL0 = 24 MHz × 40 = **960 MHz** |
| `PLLCTL.pll[0].div[0/1/2]` | `0x10000000` / `…03` / `…07` | PLL0CLK0 = 960, CLK1 = 960/1.6 = **600**, CLK2 = 960/2.4 = **400 MHz** (div = 1 + DIV/5) |
| `SYSCTL.clock_cpu` (`+0x1800`) | `0x00020101` | mux 1 = PLL0CLK0, div 1 (/2) → **CPU 480 MHz**; sub0_div 2 (/3) → **AHB 160 MHz** |
| `SYSCTL.global00` | `mux = 2` | clock preset select (SDK `sysctl_clock_set_preset`) |
| `SYSCTL.group0[0]` / `[1]` | bits 1..31 / 0..20 set one by one | every resource 257..308 linked to CPU0 group (SDK "add all clocks") |
| `SYSCTL.affiliate.set` | `1` | CPU0 affiliated to group 0 |
| `PCFG.dcdc_mode` | `&= ~0x00060000` | only mode bits 17–18 cleared, voltage untouched |
| `SYSCTL.clock[0]` (MCT0) | `0` | 24 MHz |
| `SYSCTL.clock[21]` (UART0) | `0` | 24 MHz XTAL, /1 |
| `SYSCTL.adcclk[0]` / `[1]` | `0` | mux 0 = **AHB** (160 MHz) for ADC0/ADC1 |
| `SYSCTL.clock[9]` (TMR0), `[10]` (TMR1), `[18]` (SPI1) | read only (frequency query) | left at reset/boot-ROM default |

Derived clocks: PWM0/PWM1/TRGM0/HDMA/ADC bus = AHB 160 MHz (a PWM period of 20000 ticks = 125 µs
confirms it). The SPI1 clock is never written by the app; with SCLK_DIV = 0 (÷2) the vendor comments
and the 12 MHz WS2812 timing imply the default 24 MHz source — **unverified on hardware**. `delay_ms`
(`0x80045108`) busy-waits on `rdcycle` against the CPU frequency stored at `gp+0x438`.

**UART0** (debug console, `0x00010090`): DLL `0x1A` (26), DLM 0, OSCR 8 → 24 MHz / (8 × 26) =
**115 385 baud, 8N1**, FIFO on (`fcrr = 0x008F0001`), RTS set, RX idle detect thr 10. Output only.

## 2. Pin mux (every IOC/PIOC write the app makes)

| Pad | Register | Value | Function |
|---|---|---|---|
| PA00 | `IOC 0xF4040000` | `2` | UART0_TXD |
| PA01 | `IOC 0xF4040008` | `2` | UART0_RXD |
| PA24, PA25 | `IOC 0xF40400C0/C8` | `0x100` | analog → **USB0 DM/DP** |
| **PA29** | `IOC 0xF40400E8` | **`5`** | **SPI1_MOSI → WS2812 chain** (`0x00010FDE`) |
| PB00, PB01 | `IOC 0xF4040100/108` | `0x100` | analog (ADC) |
| PB08..PB13 | `IOC 0xF4040140..168` | `0x100` | analog (ADC) |
| PY00..PY03 | `PIOC 0xF4118E00..E18` | `3` | hand pad to SOC (IOC) domain |
| PY00..PY03 | `IOC 0xF4040E00..E18` | `0x11` (alt 17) | PWM1_P4..P7 = mux address bits 0..3 |

No SPI1 SCLK/CS pads are muxed (data-only stream). **No GPIO is configured or driven** by the app: no
LED-power enable, no RGB power switch, no PAD_CTL (pull/drive) writes — all pads keep reset defaults.
USB VBUS/session are forced valid in `USB0.phy_ctrl0` (below) rather than sensed on a pin.

ADC channel ↔ pad (metapac pin table): IN1 = PB09, IN2 = PB10, IN3 = PB11, IN4 = PB12, IN5 = PB13,
IN11 = PB08. The app also converts **IN6 and IN7**, which metapac places on PB14/PB15 and lists PB00/
PB01 as IN15/IN14. Since the app only puts PB00/PB01/PB08–13 into analog mode and stage-0 samples the
same set {1..7, 11}, IN6/IN7 are almost certainly **PB00/PB01 on this package** (which one is which
is unconfirmed — check the HPM5361 QFN48 datasheet pin table).

## 3. Hall scan: ADC0/ADC1, PWM0, PWM1, TRGM0, GPTMR

### ADC0 / ADC1 (identical except channel list; `0x0000E61C` init, `0x00014BF6` sequence)

| Register | Value | Meaning |
|---|---|---|
| `conv_cfg1` | `0x151` (cal: `0x153`) | convert_clock_number 21 = **16-bit**; clock_divider 1 = ÷2 → ADC clk 80 MHz (÷4 during calibration) |
| `adc_cfg0` | `0xA0000000` | adc_ahb_en, sel_sync_ahb |
| `buf_cfg0` | `1` | wait_dis (no wait-for-read) |
| `adc16_config0` | `0x01F00408` after cal | bandgap+regulator on, cal_avg_cfg 7, conv_param 0x408 |
| calibration | `ana_ctrl0.startcal` ×4, params table written | stock SDK `adc16_init` calibration |
| `sample_cfg[ch]` | `1` | sample_clock_number 1, shift 0 (minimum) for every used channel |
| `prd_cfg[ch].prd_thshd_cfg` | `0xFFFF0000` | period-mode thresholds wide open (unused) |
| `seq_cfg0` | `0xF09` | seq_len 16, **cont_en** (one trigger runs the whole queue), **hw_trig_en** (posedge) |
| `seq_que[0..15]` ADC0 | `1,1,1,1, 3,3,3,3, 5,5,5,5, 7,7,7,7` (+`seq_int_en` on #15) | channels 1, 3, 5, 7 |
| `seq_que[0..15]` ADC1 | `2,2,2,2, 4,4,4,4, 6,6,6,6, 11,11,11,11` (+`seq_int_en` on #15) | channels 2, 4, 6, 11 |
| `seq_dma_addr` | ADC0 `0x00092260`, ADC1 `0x00092160` | DLM |
| `seq_dma_cfg` | `0xF` (after `dma_rst`) | buf_len 16 words, circular (no stop_en) |
| `seq_high_cfg` | `0` | |
| `int_en` | `0x00800000` | **seq_cvc** (sequence complete) only |

Channel table in flash at `0x800422B4`: `1×4,3×4,5×4,7×4, 0×4,2×4,4×4,6×4, 11×4`; ADC0 uses entries
0–15, ADC1 entries 20–35 (the `0`-block is skipped). Each DMA word is the HPM seq-DMA format
(result 15:0, seq index 19:16, channel 24:20, cycle bit 31 — layout from the SDK, not re-checked).
Each input is converted 4× back-to-back; the app discards the first and averages three (`hardware.md`).

### PWM0 → TRGM0 → ADC trigger (`0x00012F40`)

PWM0 counts at AHB 160 MHz; all values below are register values (`>>4` = ticks, low 4 bits fractional).

- `rld = 0x0004E1F0` → 19999 → **period 20000 ticks = 125 µs = 8 kHz**.
- `cmp[0..21]` = pairs `(320 + 1818.18·k, 420 + 1818.18·k)` for k = 0..10 (`0x1400,0x1A40, 0x85A0,0x8BE0,
  … 0x48440,0x48A80`), `cmpcfg = 0`.
- `chcfg[8] = 0x15000000` (cmpsel 0..21) → CH8REF toggles at every compare: **11 pulses per period,
  rising 2.0 µs after each mux step boundary, 0.625 µs wide**.
- `TRGM0.trgocfg[32] = trgocfg[33] = 0x30`: input `PWM0_CH8REF` → outputs `ADC0_STRGI`/`ADC1_STRGI`.
  So both ADCs start their 16-conversion sequence on each rising edge, simultaneously.
- `shlk = 0x80000000` (shadow lock). Unlock key written before every shadow write: `0xB0382607`.

### PWM1 → analog-mux address on PY00..PY03 (`0x0000EA98`)

Same period (`rld 0x4E1F0`). Each output channel toggles on its compare list and restarts low at reload:

| Output | `chcfg` (cmpsel) | Compares (ticks) = step boundaries | Bit pattern over steps 0..10 |
|---|---|---|---|
| P4 → PY00 (A0) | `[4]` 0..9 | 0, 1818, 3636, 5454, 7272, 10908, 12726, 14544, 16362, 18180 | 1 0 1 0 1 1 0 1 0 1 0 |
| P5 → PY01 (A1) | `[5]` 10..14 | 1818, 5454, 10908, 14544, 18180 | 0 1 1 0 0 0 1 1 0 0 1 |
| P6 → PY02 (A2) | `[6]` 15..17 | 5454, 9090, 14544 | 0 0 0 1 1 0 0 0 1 1 1 |
| P7 → PY03 (A3) | `[7]` 18 | 9090 | 0 0 0 0 0 1 1 1 1 1 1 |

→ addresses 1,2,3,4,5,9,10,11,12,13,14 (as in `hardware.md`). `pwmcfg[4..7] = 0x16E00000` (oen,
frcshdwupt, faultmode 2, faultrectime 3, frcsrcsel 1). Note the step-5 boundary is placed at 9090
(= 5 × 1818) for A2/A3 but A0/A1 have no edge there because their bits do not change.

Start (`0x00015920`): `PWM0.sta = PWM1.sta = 0`, then `PWM0.gcr.cen`, `PWM1.gcr.cen` in two consecutive
stores (not hardware-synchronised; skew is a few AHB cycles).

**Timeline per 125 µs:** step k (k = 0..10) = mux address set at t = 1818·k ticks (11.36 µs), ADC
trigger at +320 ticks (2 µs settling), 16 conversions per ADC (≈ 16 × 22 ADC clocks ≈ 4.4 µs at
80 MHz, estimate), then `seq_cvc` interrupt. 11 steps × 8 inputs = 88 slots per scan, 8000 scans/s.

### Interrupt chain and GPTMR

| IRQ | Source | PLIC priority | Handler / role |
|---|---|---|---|
| 58 | ADC0 seq_cvc | 5 | `0x000002A0`: counts steps, marks step ready, kicks GPTMR1 |
| 59 | ADC1 seq_cvc | 5 | `0x000003A6`: same for ADC1 |
| 6 | GPTMR1 ch0 reload | 2 | `0x0000057A`: stops GPTMR1, clears flag, runs `0x00007A88(step)` for every pending step (flags `gp+0x16C[0..10]`) |
| 51 | USB0 | 3 | CherryUSB `usb_dc` ISR |

PLIC runs in **vectored + preemptive** mode (`PLIC.feature = 3`), threshold 0. GPTMR1 is a
deferred-work "software interrupt": init `0x00015396` sets `ch0 rld = 1`, `cmp = 0`, `cr = 0x208`,
`irqen = 1`; the ADC ISRs call `0x00015250`, which pulses `cr.cntrst` (bit 14) and sets `cen` → the
reload IRQ fires ~2 timer ticks later at the lower priority, so per-step key processing runs
preemptible by the next ADC interrupt.

**GPTMR0** (`0x00015704`): ch0 `rld = 0xFFFFFFFE`, `cr = 0x208`, no IRQ; started/stopped by
`0x0001633E`/`0x0001634C`; `0x000162DE` returns `cnt / (f_TMR0/1e6)` = **µs timestamp**.
HDMA, SPI1, UART0 run without interrupts (polled).

## 4. RGB: SPI1 + HDMA for 85 WS2812

**SPI1** (`0x00010F66` init, `0x00013140`/`0x000140E0` per transfer):

| Register | Value | Meaning |
|---|---|---|
| `timing` | `0x00003B00` | sclk_div 0 → SCLK = f_SPI1/2 (12 MHz if f_SPI1 = 24 MHz), cs2sclk 3, csht 11 |
| `trans_fmt` | `0x00010F03` | **16-bit words** (datalen 15), CPOL=1 CPHA=1, addrlen field 1 (unused) |
| `trans_ctrl` | `0x051F81FF` | transmode 5 (*write, dummy, read*), wrtrancnt field 0x1F8, rdtrancnt 0x1FF |
| `wr_trans_cnt` / `rd_trans_cnt` | `0x7F8` (2041 words) / `0xFFFFFFFF` | |
| `ctrl` | `0x01000017` | cs_en, txdmaen, reset SPI+FIFOs |
| `cmd` | `0xFF` | write starts the transfer |

transmode 5 with an unbounded read phase looks odd; it is what the app writes before every frame
(the next frame's `cmd` write restarts the controller). For our firmware use write-only.

**DMA**: `DMAMUX.muxcfg[0] = 0x8000002B` (enable, request 0x2B = SPI1 TX per the HPM5300 DMAMUX map
— not in metapac, inferred) → **HDMA ch0**: `src 0x00006090` (ILM), `dst 0xF007402C` (SPI1 DATA),
`tran_size 0x7F9` (2041 × 16 bit), `ctrl 0x00252010` (src/dst 16-bit, dst fixed + handshake, half-IRQ
masked, no IRQ used). Refresh `0x00013D78`: rebuild buffer (`0x000139FC`), reprogram SPI and DMA,
set `enable`, **busy-wait for `inttcsts` bit 0**, clear `enable`. A frame therefore blocks the caller
for 2041 × 1.333 µs ≈ **2.7 ms**.

**Bit encoding** (table built at `0x00010F66` in ILM `0x5090`, 256 bytes × 8 words, MSB first):
`1` → **`0xFFC0`** (10 high = 833 ns), `0` → **`0xE000`** (3 high = 250 ns) at 12 MHz; 1.333 µs/bit.
Buffer `0x6090..0x7081`: 85 LEDs × 24 words, then **one zero word** (word 2040); the reset/latch
low time is that word plus the idle gap between frames (MOSI rests low). The frame source is a
3-byte-per-LED array at ILM `0x7970`; bytes are emitted in order **[1], [0], [2]** (GRB on the wire
if the array is RGB).

Cadence: the refresh is called from the main loop / effect code, not from an interrupt; the exact
steady-state rate was not pinned down (emulated loop: one refresh per ~830 scans ≈ 10 Hz during the
boot phase — treat as unknown). Other users: all-off + refresh `0x0001581A`, factory-wipe blink.

## 5. USB (CherryUSB on USB0, HS)

Init `0x0001137A` → `usbd_initialize` (`0x0001643C`): PHY reset sequence on `otg_ctrl0`/`phy_ctrl1`,
`phy_ctrl0 = 0x001030E3` (**VBUS/session valid overridden**), `usbcmd.rst`, `usbmode = device`,
`portsc1` PFSC clear (high speed allowed), `otgsc.vd`, `endptlistaddr = 0x00093000` (dQH),
`usbintr = 0x47` (UE, UEE, PCE, URE — **no SLE/suspend, no SOF**), `usbcmd.rs`. PLIC IRQ 51 prio 3.

### Descriptors (flash)

Device `0x80042164`: `12 01 00 02 00 00 00 40 B0 36 7E 35 02 00 01 02 03 01` — USB 2.00, class in
interface, EP0 64, VID 0x36B0, PID 0x357E, bcdDevice 0.02, strings 1/2/3, 1 configuration.
Device qualifier `0x8004222A`: `0A 06 00 02 00 00 00 40 01 00`.

Configuration `0x80042176` (116 bytes):
```
09 02 74 00 04 01 00 20 FA        config 1, 4 interfaces, attr 0x20 (bus powered, REMOTE WAKEUP), 500 mA
09 04 00 00 01 03 01 01 00        IF0 HID boot keyboard
09 21 11 01 00 01 22 3D 00          HID 1.11, report desc 61
07 05 81 03 40 00 01                EP1 IN interrupt 64 B, bInterval 1 (125 µs)
09 04 01 00 01 03 00 00 00        IF1 HID (consumer)
09 21 00 01 00 01 22 50 00          HID 1.00, report desc 80
07 05 83 03 03 00 01                EP3 IN interrupt 3 B, 125 µs
09 04 02 00 02 03 00 00 00        IF2 HID vendor
09 21 11 01 00 01 22 24 00          report desc 36
07 05 82 03 00 04 01                EP2 IN interrupt 1024 B, 125 µs
07 05 03 03 00 04 01                EP3 OUT interrupt 1024 B, 125 µs
09 04 03 00 01 03 01 02 00        IF3 HID boot mouse
09 21 11 01 00 01 22 32 00          report desc 50
07 05 84 03 03 00 01                EP4 IN interrupt 3 B, 125 µs
```
Strings (`0x800421EA`): 0 = `04 03 09 04` (en-US); 1 = "Wob Lab"; 2 = "ZENT  75 HE " (two spaces,
trailing space); 3 = "20240728" (fixed — **not** the chip UID).

### Report descriptors (no report IDs anywhere)

Keyboard `0x800420D4` (61 B): `05 01 09 06 A1 01 75 01 95 08 05 07 19 E0 29 E7 15 00 25 01 81 02
95 78 75 01 15 00 25 01 05 07 19 00 29 77 81 02 15 00 25 01 95 05 75 01 05 08 19 01 29 05 91 02 95 01
75 03 91 03 C0` → **16-byte NKRO input**: byte 0 = modifiers E0..E7, bytes 1..15 = bitmap of usages
0x00..0x77 (bit u%8 of byte 1+u/8); output 1 byte = 5 LEDs + 3 pad. Sent from `0x00093608`, length 16.

Consumer `0x80040A1C` (80 B): `05 0C 09 01 A1 01` + 24 usages `E9 EA E2 CD B7 B6 B5 192 18A 194 221
223 224 226 225 227 22A 19F 183 6F 70 2A0 29F 1CB` + `15 00 25 01 75 01 95 18 81 02 C0` → **3-byte
bitmap**, bit i (LSB of byte 0 first) = i-th usage above: Vol+, Vol−, Mute, Play/Pause, Stop,
Prev, Next, AL Calculator, AL Mail, AL My Computer, AC Search, AC Home, AC Back, AC Stop, AC Forward,
AC Refresh, AC Bookmarks, 0x19F, 0x183, Brightness+ (0x6F), Brightness− (0x70), 0x2A0, 0x29F, 0x1CB. Sent from `0x00093600`.
No System Control (power/sleep) collection exists.

Vendor `0x800409F8` (36 B): `06 60 FF 09 61 A1 01 05 62 19 00 2A FF 00 96 00 04 75 08 81 02 05 63 19 00
2A FF 00 96 00 04 75 08 91 02 C0` → 1024-byte input and 1024-byte output, usage page 0xFF60/0x61.

Mouse `0x80041C68` (50 B): `05 01 09 02 A1 01 09 01 A1 00 05 09 19 01 29 05 15 00 25 01 95 05 75 01
81 02 95 01 75 03 81 03 05 01 09 30 09 31 15 81 25 7F 75 08 95 02 81 06 C0 C0` → **3 bytes**: buttons
1–5, X, Y (relative, i8). No wheel. Sent from `0x00093604`.

### Endpoint handling

| EP | Callback | Behaviour |
|---|---|---|
| 0x81 / 0x83 / 0x84 IN | `0x000163EE` / `0x000163E4` / `0x000163DA` | clear the **one shared busy flag** `0x00088242` |
| 0x82 IN | `0x00015FCC` | if length is a non-zero multiple of 1024 → send ZLP, else clear `0x00088242` |
| 0x03 OUT | `0x00015DE6` | re-arm 1024-byte read into `0x00093618`, store length in `gp+0x3F8` (main loop dispatches) |

All four IN endpoints share one in-flight flag, so at most one report of any kind is queued at a time;
senders spin on `0x00088242` (e.g. `0x0000EA70`). Vendor replies are built in `0x00092A18`.

### HID class requests (`0x00011A7A`, jump table `0x800401D4`, any interface)

| bRequest | Behaviour |
|---|---|
| GET_REPORT (1) | returns 1 byte `00` |
| GET_IDLE (2) | returns 1 byte `00` |
| GET_PROTOCOL (3) | returns `00` (= boot!) regardless of state |
| SET_REPORT (9) | if wLength = 1: **Caps Lock** = `data[0] & 0x02` stored at `0x0009162B` (`gp−0x601`), counter `gp+0x49C`++; other LED bits ignored |
| SET_IDLE (0x0A), SET_PROTOCOL (0x0B) | accepted, ignored |
| others | logged "unhandled", STALL |

Consequence: the boot-keyboard interface always sends the 16-byte NKRO report, even to a host that
selected boot protocol (BIOS/UEFI may not understand it).

### Suspend / remote wakeup

No suspend interrupt is enabled. The main loop polls `USB0.portsc1.ls` bit 1 (`0x00007E72`) and
counts 16000 consecutive scans (2 s) before entering its idle/sleep state (`gp+0x444`, `gp+0x3AC`).
Remote wakeup (`0x00008256`): when a key event arrives in that state the app sets `portsc1.fpr`
(force port resume), waits 10 ms, clears it, waits, then sends the keyboard report. The exact sleep
state machine (RGB off, `sleep_second_cnt`) belongs to `algorithms.md`; this part is **partially
decoded**.

## 6. Flash (XPI0 NOR through the ROM API)

API table `0x2001FF00`; `xpi_nor_driver_if` = `*(0x2001FF14)`; all calls pass `base = XPI0
(0xF3000000)`, `channel = 4` (`xpi_xfer_channel_auto`), config struct at DLM `0x0009132C`.

| Slot (offset in driver if) | Used for | Called from |
|---|---|---|
| `+0x4C` auto_config | option `{0xFCF90002, 0x00000006, 0x00001000}` | `0x00013E8C` |
| `+0x50` get_property | id 0 (total size) → `gp+0x46C`, id 2 (sector size) → `gp+0x3D4`, id 1 (page size) → `gp+0x408` | `0x00013E8C` |
| `+0x18` erase(start, len) | | `0x00014A44`, `0x00015A48` |
| `+0x28` program(src, dst, len) | | `0x00014A44` |
| `+0x2C` read(dst, start, len) | | `0x0001598C` |

`0x00013E8C` (auto_config + get_property) runs **before every** erase/program/read. Every write is
erase-then-program of the whole region (no read-modify-write), followed by `fence.i`; interrupts stay
enabled (the code runs from ILM). Sector size comes from get_property (4 KB on standard NOR; ROM erase
rounds the range up to whole sectors).

| Flash (XPI offset) | Length | RAM shadow (ILM) | Content / when |
|---|---|---|---|
| `0x80070000` (`0x70000`) | `0x790` | `0x7084` | bank 1, board-global; rewritten after bank-1 `BE 03` writes and when the firmware changes board-global fields (`0x0000DE5A` at init, `0x00015F5A`) |
| `0x80080000 + k·0x2000` | `0x1000` + `0xE38` | `0x0658` (+`0x1000`) | profile k = 0..3 (`gp+0x470 = (cur_param_num + 0x40) << 13`); loaded at boot/switch (`0x000146AE`), saved by `BE 02` (`0x0000DF88`, `0x0001388C`, `0x0001555A`) |
| `0x80070000..0x80087FFF` | erase `0x18000` | — | factory wipe (`0x0000831C`): disable USB IRQ, erase, LED blink ×3, watchdog reset |

So the NOR bank **is a 1:1 image of the ILM shadow**: bank 1 = 1936 bytes, each profile = 7736 bytes
in its own 8 KB (2-sector) slot. Nothing outside `0x80070000..0x80087FFF` is ever erased or programmed.

## 7. Reset and bootloader jump

**`BE FF` / reset** (`0x00015C9C`): `PPOR.software_reset (0xF410001C) = 6 000 000` → reset after
6 M × (1/24 MHz) = **250 ms**; prints "Waiting for WDG reset...", `delay_ms(1000)`. (It is the PPOR
software-reset counter, not a watchdog.) Also used by the factory wipe and keycode 319 path.

**`BE FC`** (`0x00015A0C`):
1. puts "jump to download mode ...".
2. `0x00012A46`: PLIC disable IRQ 51 (USB0), 6 (GPTMR1), 58, 59 (ADC); **PWM0 fully de-initialised**
   (rld/cmp = `0x0FFFFFF0`, chcfg = `0x17170000`, cmpcfg/pwmcfg/gcr/shcr = 0 — PWM1 is left running);
   ADC0/ADC1 `int_en = 0`; USB `usbd_deinit` + PLIC IRQ 51 off (`0x000161BC`); HDMA ch0 disable;
   `delay_ms(1000)`.
3. Andes CCTL `mcctlcommand (0x7CC) = 7` (L1D write-back all); clear `mcache_ctl (0x7CA)` bit 1
   (D-cache off) then bit 0 (I-cache off); `fence.i`.
4. `jr 0x80020000` (download loader). No stack/CSR reset, `mstatus.MIE` untouched.

**Sleep / low power**: no WFI, no SYSCTL low-power or clock gating at runtime was found; "sleep" is
an application state (see §5), the CPU keeps scanning at 8 kHz.

## 8. Chip UID (`BE 1D`)

`0x0001550E`: `otp_driver_if = *(0x2001FF0C)`; calls `init()` (`+0x04`), then `read_from_shadow(w)`
(`+0x0C`) for w = 0x58, 0x59, 0x5A, 0x5B; the four u32 are stored at `gp+0x318` (`0x00091F44`) and
returned little-endian as the 16 UID bytes. The USB serial string is the fixed "20240728", not this.

## Open questions

- f_SPI1 (reset default vs. boot-ROM/stage-0 setting) → confirm 12 MHz SCLK on hardware or set it.
- Which of PB00/PB01 is ADC IN6 vs IN7 (metapac's table disagrees with the firmware).
- DMAMUX request `0x2B` = SPI1 TX assumed from the SDK map.
- Steady-state RGB refresh rate and the full suspend/sleep state machine.
