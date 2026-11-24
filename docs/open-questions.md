# Open questions and physical-access items

> For the exact capture list to close the physical items below, see [`physical-access-checklist.md`](physical-access-checklist.md).

What the static reverse-engineering could not settle, grouped by whether it needs hardware. The
confident findings are in `hardware.md`, `protocol.md`, `algorithms.md`, `bootloader.md`,
`stable-vs-latest.md`.

> **Status (2026-11-24): this IS the sanctioned terminal state for the physical items.** GOAL.md's
> Phase 0 gate says, verbatim, that if custom firmware cannot be safely flashed without an
> app-independent recovery path, the session must "stop and list physical-access needs in
> docs/open-questions.md." Every software-reachable question has been answered or narrowed to the
> single fact that needs a multimeter/photo; the items below are that list. They are **deferred to
> physical access, not unresolved analysis** — nothing more can be learned about them from the
> firmware, dumps, or the device over USB. The exact capture list to close them is
> [`physical-access-checklist.md`](physical-access-checklist.md). No other part of the project is
> blocked on them: the rollback gate passed, the panel is verified, and the Rust skeleton compiles.

## Determinable only with physical access

- **Exact MCU part and package.** Narrowed to **a QFN48 HPM5361-class part (`HPM5361IEG1`)** from the
  datasheet (16 analog pins total, 8 bonded on QFN48; package-suffix decode `EG` = QFN48, `CB` =
  LQFP100, `CF` = LQFP64) and the scan's 8 sense lines (`hardware.md §1`). The exact functional SKU
  (HPM5361 full / HPM5331 motion / HPM5321 comms — all 2-ADC, same die/pinout, indistinguishable from
  firmware) and the package for certain can only be confirmed by **reading the die silkscreen** — open
  the case. (HPM5301 is ruled out: it has only ADC0, but the firmware drives both ADC0 and ADC1.)
- **Analog multiplexer chip and its select lines.** The 8 ADC **sense-input** pads are now pinned
  from the binary — **PB00, PB01, PB08–PB13**, set to analog mode by `FUN_ram_00015f08` (channels
  0–7; `hardware.md §3`). What remains is the mux IC and its **select** lines: the scan steps 8 sense
  lines through 11 positions, driven by a GPTMR1 pulse (`FUN_ram_00015250`), with **no steady-state
  GPIO writes to the select lines** in the firmware. The four `PIOC` pads at `0xf4118e00/e08/e10/e18` = **PY00–PY03**
  (func `3` = SOC_GPIO_Y, set once in `FUN_ram_00015c3a`) are the most likely 4-bit address lines, but the mux IC itself (the
  Rainy75-HE sibling reportedly uses Nexperia 74HC4067-class 16:1 parts) and the pad→mux-input wiring
  need the **schematic or a logic analyzer**, plus continuity to confirm the `0x80040528`/`0x80040580`
  scan-order tables against the physical matrix.
- **RGB LED data pin.** Driven by SPI1 MOSI via an IOC pad (func 5, register `0xf40400e8`);
  the exact package pin needs the IOMUX decode against the HPM5361 QFN48 pinout, or probing.
- **RESOLVED (2026-11-24): the signature-based recovery holds.** Confirmed by device experiment
  (`tools/confirm_sig_recovery.py`): an app with word0 ≠ `0x0b4d5475` is refused and the board stays
  in the TinyUF2 drive (`34b7:1001`) on reset — app-independent, no hardware. See `bootloader.md §3`.
- **STILL OPEN — which pad is the TinyUF2 reset-time GPIO trigger (for a signature-VALID hung app).**
  The signature lever only forces the loader for a *freshly flashed* signature-less image; it does not
  rescue an already-running signed app that hangs. The *mechanism* is now known from the TinyUF2 source
  (`hpm_sdk/samples/tinyuf2`, `bootloader.md §5`): one **active-high GPIO sampled at reset** (held high
  → app, pulled low → DFU) plus the signature check — and **double-tap is ruled out** (this HPM port
  has no no-init-RAM magic). What remains is **which physical pad the vendor wired as that trigger and
  whether it is accessible** — the loader binary at `0x80020000` is not in any software-reachable dump
  (`BE 01` validates only `0x80080000..0x80100000`; ROM ISP / JTAG need the pads below). Needs the
  **case opened** to find/probe the pad. Until then, only flash custom builds that can be made
  signature-invalid for recovery.
- **Recovery pads to locate and label before any custom flash** (`bootloader.md §5`): UART0 ROM-ISP
  pads **PA00 (TXD) / PA01 (RXD)**, the **BOOT pins PA02/PA03**, and **JTAG PA04–PA08**. Note this is
  a RISC-V core: debug is **JTAG, not SWD**. Whether JTAG is **OTP-locked** on this unit is unknown
  and can only be checked by attaching a probe.

## Determinable from the binary with more analysis (not yet traced)

- **DKS byte 47 ("DKSOriginCover" / "trigger the original key").** Parsed into the runtime struct in
  `FUN_ram_0000a6f8`, but its exact effect — whether it suppresses or still emits the physical key's
  own code — was not traced through the actuation path (`FUN_ram_00009d10` / `FUN_ram_0000e31c`). The
  vendor side is itself ambiguous (variable name "cover" vs UI label "trigger"). Needs a deeper trace
  of the DKS state machine, or an on-device A/B test.
- **`rgb_config` low two bits always written as 1** (vendor observation). Meaning not confirmed on
  the firmware side; trace the `rgb_matrix` config consumer that reads `0x80080C60`.
- **Unnamed config bytes `0x80080C65..0x80080C67`.** Carried in the vendor's 17-byte RGB export but
  never named; firmware meaning unconfirmed.
- **FD-stream tail semantics.** The 1017 tail words beyond the scan-rate (`8000`) — `0x91fb0/1000`
  (≈680) and the 8 bytes from `0x843e8` — are internal counters whose exact meaning was not pinned
  down (`protocol.md §BE FD`). The two auxiliary per-key arrays streamed at words `[332..497]`
  (`0x90f28`, `0x8f3a8`, zero at rest) are identified as velocity/diagnostic arrays but their precise
  formula was not fully derived.
- **The `0xDA` diagnostic post-processor** `FUN_ram_00015a84(&0x801b0, &0x8f248, &0x8f3a8, &0x8f0e8)`
  rewrites the travel/auxiliary arrays from a table at `0x801b0`; the exact transform (and why the
  vendor ships but never uses it) was not fully decoded.

## Product / process questions

- **Travel-stuck-at-0 reproduction.** The root cause is understood (`algorithms.md §8`). Still worth
  doing on-device: dump a victim key's live `BE FD` raw range vs its stored `top_value`/`bot_value`
  to quantify how small the real swing is relative to the 31000/17000 default, and confirm the panel
  can flag such keys automatically (stored `top−bot` implausibly large vs the live raw span).
- **Custom-firmware recovery drill — DONE (2026-11-24).** `tools/confirm_sig_recovery.py` ran the
  full drill: flashed a signature-invalid image, confirmed the board returned to TinyUF2 on reset
  (app refused), restored vendor 0.0.18 and verified the config byte-identical. Closes the loop for
  signature-less builds; the signed-but-hung case remains gated on the hardware trigger above.
