"""Zent transport against the in-process fake keyboard (socketpair, no hardware)."""
import pytest

import rammap
import zentlib as zl

FAST = dict(timeout=0.2)


def test_send_writes_report_id_zero_and_1024_bytes(rig):
    z, kb = rig
    assert z.version() == "0.0.18"
    assert len(kb.requests[0]) == 1025 and kb.requests[0][0] == 0
    assert kb.requests[0][1:12] == bytes.fromhex("be040b00100207800004ed")


def test_read_ram_returns_exact_bytes(rig):
    z, kb = rig
    kb.mem_profiles[0][0x160:0x160 + 166] = bytes(range(166))
    assert z.read_ram(rammap.KEY_MAP, 166) == bytes(range(166))
    assert z.read_ram(rammap.KEY_MAP + 2, 3) == bytes([2, 3, 4])


def test_read_retries_after_lost_and_corrupt_replies(rig):
    z, kb = rig
    kb.mem_global[0:4] = b"\x18\x79\x18\x79"
    kb.drop_replies = 1
    assert z.read_ram(rammap.TOP_VALUE, 4, **FAST) == b"\x18\x79\x18\x79"
    kb.corrupt_replies = 2
    assert z.read_ram(rammap.TOP_VALUE, 4, **FAST) == b"\x18\x79\x18\x79"
    kb.drop_replies = 3
    with pytest.raises(zl.ZentError):
        z.read_ram(rammap.TOP_VALUE, 4, **FAST)


def test_read_ignores_stale_reply_for_other_address(rig):
    z, kb = rig
    kb.mem_global[0:4] = b"\xAA\xBB\xCC\xDD"
    kb.stale_before_reply = True
    assert z.read_ram(rammap.TOP_VALUE, 4) == b"\xAA\xBB\xCC\xDD"


def test_read_rejects_short_reply(rig):
    z, kb = rig
    kb.short_replies = True
    with pytest.raises(zl.ZentError):
        z.read_ram(rammap.TOP_VALUE, 4, tries=2, **FAST)


def test_read_ram_big_chunks_and_reassembles(rig):
    z, kb = rig
    image = bytes((i * 31) & 0xFF for i in range(48 * 83))
    kb.mem_profiles[0][0xD00:0xD00 + len(image)] = image
    assert z.read_ram_big(rammap.BIND_KEY_SETTING, len(image)) == image
    reads = [r for r in kb.requests if r[2] == zl.CMD_READ_RAM]
    sizes = [int.from_bytes(r[9:11], "big") for r in reads]
    assert sum(sizes) == len(image) and max(sizes) == zl.MAX_CHUNK and sizes[-1] == len(image) % zl.MAX_CHUNK
    addrs = [int.from_bytes(r[5:9], "little") for r in reads]
    assert addrs == [rammap.BIND_KEY_SETTING + i * zl.MAX_CHUNK for i in range(len(reads))]
    assert z.read_ram_big(rammap.BIND_KEY_SETTING, 100, chunk=7) == image[:100]


def test_requests_outside_the_map_are_refused_before_any_io(rig):
    z, kb = rig
    for call in (lambda: z.read_ram(0x80000000, 4), lambda: z.read_flash(0x80070000, 167),
                 lambda: z.read_ram_big(0x80070000, 400), lambda: z.write_ram(0x80090000, b"\x00"),
                 lambda: z.write_ram(rammap.SOFT_VERSION, bytes(4)),
                 lambda: z.write_ram(rammap.CUR_PARAM_NUM, bytes(4))):
        with pytest.raises(ValueError):
            call()
    assert kb.requests == []
    assert z.read_ram(0x80070208, 4, unchecked=True) == bytes(4)  # explicit override


def test_write_ram_round_trip_and_wire_format(rig):
    z, kb = rig
    data = bytes((i * 7 + 1) & 0xFF for i in range(166))
    z.write_ram(rammap.KEY_MAP, data)
    assert bytes(kb.mem_profiles[0][0x160:0x160 + 166]) == data
    assert kb.mem_profiles[0][0x160 + 166] == 0 and kb.mem_profiles[0][0x15F] == 0  # nothing spilled
    frame = kb.requests[0][1:]
    assert frame[:4] == bytes([0xBE, 0x03]) + (13 + 166).to_bytes(2, "little")
    assert frame[8:10] == (166).to_bytes(2, "big") and frame[10:176] == data
    assert int.from_bytes(frame[176:178], "little") == zl.crc16(frame[:176]) and frame[178] == 0xED
    assert z.read_ram(rammap.KEY_MAP, 166) == data


def test_write_ram_splits_large_blocks(rig):
    z, kb = rig
    image = bytes((i * 13 + 5) & 0xFF for i in range(48 * 83))
    z.write_ram(rammap.BIND_KEY_SETTING, image)
    assert bytes(kb.mem_profiles[0][0xD00:0xD00 + len(image)]) == image
    writes = [r for r in kb.requests if r[2] == zl.CMD_WRITE_RAM]
    assert len(writes) == -(-len(image) // zl.MAX_CHUNK)


def test_write_without_ack_raises(rig):
    z, kb = rig
    kb.drop_replies = 1
    with pytest.raises(zl.ZentError, match="no ack"):
        z.write_ram(rammap.KEY_MODE, bytes(83), timeout=0.2)


def test_write_verify_catches_an_acked_but_unapplied_write(rig):
    z, kb = rig
    kb.ignore_writes = True
    with pytest.raises(zl.ZentError, match="verify failed"):
        z.write_ram(rammap.KEY_MODE, b"\x01" * 83)
    z.write_ram(rammap.KEY_MODE, b"\x01" * 83, verify=False)  # the ack alone cannot tell


def test_profile_switch_swaps_memory_and_is_confirmed(rig):
    z, kb = rig
    for p in range(4):
        kb.mem_profiles[p][0x6E0] = 0x40 + p
    for p in (2, 0, 3, 1):
        z.switch_profile(p, settle=0.01)
        assert z.profile() == p and z.read_ram(rammap.KEY_MODE, 1)[0] == 0x40 + p
    with pytest.raises(ValueError):
        z.switch_profile(4)


def test_profile_switch_that_does_not_happen_raises(rig):
    z, kb = rig
    kb.ignore_profile_switch = True
    with pytest.raises(zl.ZentError, match="not confirmed"):
        z.switch_profile(2, settle=0.01, timeout=0.3)
    z.switch_profile(2, settle=0.01, verify=False)  # old blind behaviour stays available


def test_serial_and_save(rig):
    z, kb = rig
    assert z.serial() == bytes([0xBE, 0x1D, 0x15, 0x00]) + kb.serial + b"\xED"
    z.write_ram(rammap.KEY_MODE, b"\x05" * 83)
    assert z.save(settle=0.05) is None  # the firmware sends no ack
    assert kb.saves == 1
    assert z.read_flash(rammap.KEY_MODE, 83) == b"\x05" * 83


def test_drain_is_bounded_under_a_continuous_stream(rig):
    z, kb = rig
    frame = bytes([0xBE, 0xFD, 0xF9, 0x03]).ljust(1024, b"\x00")
    for _ in range(40):
        kb.sock.send(frame)
    z.drain(limit=0.2)
    assert z.version() == "0.0.18"


def test_context_manager_closes(rig):
    z, _ = rig
    with z:
        pass
    assert z.fd == -1
    z.close()  # idempotent


def test_debug_commands_are_never_sent_to_stock(rig):
    z, kb = rig  # the fake reports 0.0.18
    for _ in range(2):
        with pytest.raises(zl.ZentError, match="not ours"):
            z.debug_request(zl.CMD_DEBUG_CONSOLE, 0)
    assert kb.debug_requests == 0
    assert all(r[2] == zl.CMD_READ_RAM for r in kb.requests)


def test_debug_request_round_trip_on_our_firmware(rig):
    z, kb = rig
    kb.mem_global[0x210:0x214] = bytes([18, 0, 1, 0])  # 1.0.18
    kb.console = b"scan 8000/s\r\n" * 100
    total, start, text = z.debug_request(zl.CMD_DEBUG_CONSOLE, 14)
    assert (total, start) == (1300, 14) and text == kb.console[14:14 + 1011]
    assert kb.requests[-1][1:14] == bytes.fromhex("bed00d00") + b"ZDBG" + bytes.fromhex("0e000000ed")


def test_debug_frame_codec():
    f = zl.encode_debug(zl.CMD_DEBUG_PROF_READ, 0x01020304)
    assert f == bytes.fromhex("bed10d005a44424704030201ed")
    reply = bytes.fromhex("bed1110002000000010000000a0b0c0ded").ljust(1024, b"\x00")
    assert zl.decode_debug(reply, zl.CMD_DEBUG_PROF_READ) == (2, 1, bytes.fromhex("0a0b0c0d"))
    assert zl.decode_debug(reply, zl.CMD_DEBUG_CONSOLE) is None
    assert zl.decode_debug(zl.WRITE_ACK.ljust(1024, b"\x00"), zl.CMD_WRITE_RAM) is None
    assert zl.is_our_firmware(bytes([18, 0, 1, 0])) and not zl.is_our_firmware(bytes([18, 0, 0, 0]))
