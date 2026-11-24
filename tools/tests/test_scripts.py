"""The command-line scripts end to end against the fake keyboard (no hardware)."""
import json
import sys

import pytest

import rammap
import zentlib as zl
from conftest import stream_report

N = rammap.KEY_COUNT


def run(monkeypatch, module, rig, *argv):
    """Run module.main() with its Zent bound to the fake keyboard."""
    z, _ = rig
    monkeypatch.setattr(module, "Zent", lambda dev=None: z)
    monkeypatch.setattr(sys, "argv", [module.__name__, *argv])
    module.main()


def test_zent_read_output(monkeypatch, capsys, rig):
    import zent_read
    _, kb = rig
    kb.mem_global[0x218] = 2
    kb.active = 2
    main_layer = [41, -8, 401, 1000, 999] + [4] * (N - 5)
    kb.mem_profiles[2][0x160:0x160 + 2 * N] = b"".join(v.to_bytes(2, "little", signed=True) for v in main_layer)
    kb.mem_profiles[2][0x6E0:0x6E0 + N] = bytes([1] * N)
    run(monkeypatch, zent_read, rig)
    out = capsys.readouterr().out.splitlines()
    assert out[0] == "firmware 0.0.18"
    assert out[1] == "active config 2"
    assert out[2] == "== main layer (83 keys)"
    assert out[3].startswith("0:Esc 1:LCtrl 2:FN 3:(empty) 4:999 5:A ")
    assert out[4] == "== fn layer (83 keys)" and out[5].startswith("0:NONE 1:NONE")
    assert out[6] == "== key_mode " + str([1] * N)
    assert all(r[2] == zl.CMD_READ_RAM for r in kb.requests)  # read-only


def test_zent_all_flags_dead_key(monkeypatch, capsys, rig):
    import zent_all
    _, kb = rig
    rest = [30000] * N
    pressed = list(rest)
    pressed[59], pressed[73] = 18000, 18500  # LShift moves and travels, LCtrl moves but reads 0
    travel = [0] * N
    travel[59] = 3400
    kb.stream_travel = [stream_report(zl.STREAM_LEN_TRAVEL, rest + [0] * N),
                        stream_report(zl.STREAM_LEN_TRAVEL, pressed + travel)]
    run(monkeypatch, zent_all, rig, "--seconds", "0.4")
    assert capsys.readouterr().out.splitlines() == [
        "pressed keys (adc swing > 3000):",
        "LShift  adc 30000->18000  travel 3.40 mm",
        "LCtrl   adc 30000->18500  travel 0.00 mm  <-- BROKEN: sensor moved, firmware travel ~0",
    ]
    assert all(r[2] == zl.CMD_HEARTBEAT for r in kb.requests)


def _calib_rig(kb, low):
    kb.mem_global[0x000:0x000 + 2 * N] = zl.pack_u16([31000] * N)
    kb.mem_global[0x0B0:0x0B0 + 2 * N] = zl.pack_u16([17000] * N)
    scan_hi, scan_lo = [30100] * N, [30100] * N
    scan_lo[59], scan_lo[73] = low, low + 300
    kb.stream_raw6 = [stream_report(zl.STREAM_LEN_RAW6, scan_hi * 6),
                      stream_report(zl.STREAM_LEN_RAW6, scan_lo * 6)]


def test_zent_calib_writes_only_selected_keys_and_saves(monkeypatch, capsys, tmp_path, rig):
    import zent_calib
    _, kb = rig
    _calib_rig(kb, 18000)
    monkeypatch.setattr(zent_calib, "HERE", str(tmp_path))
    monkeypatch.setattr(zent_calib, "SAVE_SETTLE_S", 0.05)
    run(monkeypatch, zent_calib, rig, "--seconds", "0.4")
    top = zl.unpack_u16(bytes(kb.mem_global[0x000:0x000 + 2 * N]))
    bot = zl.unpack_u16(bytes(kb.mem_global[0x0B0:0x0B0 + 2 * N]))
    assert top == [30100 if k in (59, 73) else 31000 for k in range(N)]
    assert bot == [18000 if k == 59 else 18300 if k == 73 else 17000 for k in range(N)]
    assert kb.calibration_log == [True, False] and kb.saves == 1
    (backup,) = tmp_path.glob("zent_calib_backup_*.json")
    assert json.loads(backup.read_text()) == {"top": [31000] * N, "bot": [17000] * N}
    out = capsys.readouterr().out
    assert "before: {'LShift': (31000, 17000), 'LCtrl': (31000, 17000)}" in out
    assert "CALIBRATING for 0.4s: press LCtrl and LShift fully, several times" in out
    assert "written + verified: {'LShift': (30100, 18000), 'LCtrl': (30100, 18300)}" in out
    assert out.rstrip().endswith("saved to flash")


def test_zent_calib_refuses_small_travel(monkeypatch, capsys, tmp_path, rig):
    import zent_calib
    _, kb = rig
    _calib_rig(kb, 29000)  # only ~1100 counts of swing
    monkeypatch.setattr(zent_calib, "HERE", str(tmp_path))
    with pytest.raises(SystemExit) as e:
        run(monkeypatch, zent_calib, rig, "--seconds", "0.4")
    assert e.value.code == 1
    assert "NOT WRITTEN: too little travel seen for ['LShift', 'LCtrl']" in capsys.readouterr().out
    assert kb.saves == 0 and kb.calibration_log == [True, False]
    assert not any(r[2] == zl.CMD_WRITE_RAM for r in kb.requests)


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
