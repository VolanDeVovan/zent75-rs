# zent75-he

Our own firmware and configurator for the **HOVEY WORKS ZENT75-HE** hall-effect keyboard (sold as
"Wob Lab ZENT 75 HE", USB `36B0:357E`, 83 keys, 8 kHz). The stock vendor firmware and web driver
(https://hoveyworks-driver.com/, Chromium only) are fully reverse-engineered; the findings needed to
write a replacement are in `docs/`.

## Layout

| Path | What |
|---|---|
| `docs/hardware.md` | MCU (HPM5361), flash map, scan (ADC + mux), RGB, USB |
| `docs/protocol.md` | configurator HID protocol, RAM map, keymap codes |
| `docs/algorithms.md` | stock travel / calibration / actuation / SOCD / DKS behaviour |
| `docs/boot-recovery.md` | boot chain, flashing rules, **recovery (hold A+S+D at plug-in)** |
| `docs/firmware-plan.md` | plan and milestones for our Rust firmware |
| `firmware/` | Rust firmware (embassy + hpm-hal), packaging to UF2 |
| `firmware/vendor/` | original vendor images 0.0.18 (on the unit) and 0.0.9 — do not publish |
| `panel/` | WebHID configurator (TypeScript + Vite), see `panel/README.md`; vendor-side field semantics in `panel/PROTOCOL-NOTES.md` |
| `tools/` | Python device tools: `zentlib`/`rammap` (protocol), `dump_all`, `restore`, `flash_uf2`, `rollback.sh`, `uf2`, tests |
| `backups/` | latest full config backup of the unit (all four profiles) |

## Quick start

```sh
# configurator
cd panel && npm install && npm run dev          # open in Chrome/Edge

# firmware build → firmware/zent75-rs.uf2
cd firmware && ./package-uf2.sh

# device tools (Linux, hidraw)
python3 tools/dump_all.py                       # back up the config
tools/rollback.sh                               # stock 0.0.18 + restore latest backup
cd tools && uv run --with pytest python -m pytest -q tests
```

Rules for flashing and talking to the device: `AGENTS.md` and `docs/boot-recovery.md`.

## Device notes

- hidraw: if00 keyboard, if01 consumer, **if02 vendor protocol**, if03 mouse. On the owner's desktop
  a udev rule (`nixos/parts/modules/nixos/hardware/keyboard-zent75.nix`) makes `36b0` hidraw nodes
  0666.
- The unit runs stock 0.0.18; stay on it. Keys held while plugging in read travel 0 until a clean
  replug.
