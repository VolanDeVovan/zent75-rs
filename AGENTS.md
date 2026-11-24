# AGENTS.md

The ZENT75-HE is the owner's daily-driver keyboard: every action must leave it recoverable.
Start with `README.md`; details live in `docs/`.

## Hard rules

1. **Never write flash `0x80000000..0x8003FFFF`** (boot header, stage-0 recovery, loader) — no UF2,
   no firmware flash code. `tools/flash_uf2.py` refuses such images; never bypass it.
2. **Never send `BE 01`.** It hangs the stock firmware.
3. No exploratory commands or addresses on the live device; read the handler first.
4. Back up before any device write or flash (`tools/dump_all.py`), verify after
   (`tools/restore.py`), and leave the original profile active.
5. A hung or bricked app is recovered by the owner holding **A+S+D while plugging in**, then
   `tools/rollback.sh`. Say when you need their hands. Afterwards they must replug with no keys held.

Why, and the full procedures: `docs/boot-recovery.md`.

## Where to look

- Hardware, flash map, scan, RGB, USB → `docs/hardware.md`
- Configurator protocol, RAM map, keycodes → `docs/protocol.md`
- Stock key-processing behaviour → `docs/algorithms.md`
- Firmware plan and milestones → `docs/firmware-plan.md`, code in `firmware/`
- Panel → `panel/README.md`

## Checks

- `cd tools && uv run --with pytest python -m pytest -q tests`
- `cd panel && npx vitest run`
- `cd firmware && ./package-uf2.sh`
