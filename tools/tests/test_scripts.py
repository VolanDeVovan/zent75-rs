"""The command-line scripts end to end against the fake keyboard (no hardware)."""
import json
import sys

import rammap
import zentlib as zl

N = rammap.KEY_COUNT


def run(monkeypatch, module, rig, *argv):
    """Run module.main() with its Zent bound to the fake keyboard."""
    z, _ = rig
    monkeypatch.setattr(module, "Zent", lambda dev=None: z)
    monkeypatch.setattr(sys, "argv", [module.__name__, *argv])
    module.main()


def test_dump_all_profiles_and_restore_active(monkeypatch, capsys, tmp_path, rig):
    import dump_all
    z, kb = rig
    kb.active = 1
    kb.mem_global[0x218] = 1
    for p in range(4):
        kb.mem_profiles[p][0x6E0:0x6E0 + N] = bytes([p + 1] * N)
    monkeypatch.setattr(z, "switch_profile", lambda n, _orig=z.switch_profile: _orig(n, settle=0.01))
    run(monkeypatch, dump_all, rig, "fake", "--out", str(tmp_path))
    (directory,) = tmp_path.iterdir()
    meta = json.loads((directory / "dump.json").read_text())
    assert list(meta) == ["timestamp", "firmware", "active_profile", "serial_raw", "regions", "profiles", "sha256"]
    assert meta["firmware"] == "0.0.18" and meta["active_profile"] == 1 and kb.active == 1
    assert meta["regions"] == {k: list(v) for k, v in rammap.REGIONS.items()}
    assert meta["serial_raw"] == (bytes([0xBE, 0x1D, 0x15, 0]) + kb.serial + b"\xED").hex()
    for p in range(4):
        assert meta["profiles"][str(p)]["key_mode"]["data"] == bytes([p + 1] * N).hex()
        assert (directory / f"p{p}_key_mode.bin").read_bytes() == bytes([p + 1] * N)
    assert len(meta["sha256"]) == 4 * len(rammap.REGIONS)
    assert len((directory / "SHA256SUMS").read_text().splitlines()) == 4 * len(rammap.REGIONS)
    assert not any(r[2] in (zl.CMD_WRITE_RAM, zl.CMD_SAVE) for r in kb.requests)  # read-only


def test_dump_all_skips_bind_regions_on_old_firmware(monkeypatch, tmp_path, rig):
    import dump_all
    z, kb = rig
    kb.mem_global[0x210] = 9  # 0.0.9
    monkeypatch.setattr(z, "switch_profile", lambda n, _orig=z.switch_profile: _orig(n, settle=0.01))
    run(monkeypatch, dump_all, rig, "fake", "--out", str(tmp_path))
    (directory,) = tmp_path.iterdir()
    meta = json.loads((directory / "dump.json").read_text())
    assert not any(n.startswith("bind_key_") for n in meta["regions"])
    reads = [int.from_bytes(r[5:9], "little") for r in kb.requests if r[2] == zl.CMD_READ_RAM]
    assert max(reads) < rammap.BIND_KEY_SETTING
