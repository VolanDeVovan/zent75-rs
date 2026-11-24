"""RAM map of the ZENT75-HE stock firmware: the one place where the addresses live.

These are the addresses the vendor driver calls "RAM" (`BE 03` write / `BE 04` read), see the
table in README.md. `0x8007xxxx` is board-global, `0x8008xxxx` belongs to the active profile
and is swapped by `BE C0`.

Lengths are byte counts. The 83 x u16 tables are therefore 166 bytes long.
"""
from __future__ import annotations

KEY_COUNT = 83
LED_COUNT = 85
PROFILE_COUNT = 4

PROFILE_NAME_SIZE = 64
BIND_RECORD_SIZE = 48  # bind_key_setting: one DKS / key-group record per key

#: Key index -> legend, row by row (Esc = 0, LShift = 59, LCtrl = 73, Right = 82).
KEY_NAMES = (
    "Esc F1 F2 F3 F4 F5 F6 F7 F8 F9 F10 F11 F12 Print Home "
    "` 1 2 3 4 5 6 7 8 9 0 - = Bksp Del "
    "Tab Q W E R T Y U I O P [ ] \\ PgUp "
    "Caps A S D F G H J K L ; ' Enter PgDn "
    "LShift Z X C V B N M , . / RShift Up End "
    "LCtrl LWin LAlt Space RAlt FN RCtrl Left Down Right"
).split()
assert len(KEY_NAMES) == KEY_COUNT

# Addresses the tools refer to by name.
TOP_VALUE = 0x80070000
BOT_VALUE = 0x800700B0
SOFT_VERSION = 0x80070210
CUR_PARAM_NUM = 0x80070218
CONFIG_NAME = 0x80070290
KEY_MAP = 0x80080160
KEY_MAP_FN = 0x80080210
KEY_MODE = 0x800806E0
BIND_KEY_SETTING = 0x80080D00
BIND_KEY_MODE = 0x80081DE0

#: First firmware patch level that has bind_key_setting / bind_key_mode (the vendor driver
#: reads them only when the version is >= 18).
ADVANCED_KEYS_MIN_VERSION = (0, 0, 18)

#: name -> (address, byte length). Insertion order is the dump order of tools/dump_all.py and
#: the key order of backups/<ts>/dump.json, so do not reorder.
REGIONS: dict[str, tuple[int, int]] = {
    # ---- board-global
    "top_value": (TOP_VALUE, 2 * KEY_COUNT),
    "bot_value": (BOT_VALUE, 2 * KEY_COUNT),
    "keytype_code": (0x80070160, 2 * KEY_COUNT),
    "soft_version": (SOFT_VERSION, 4),
    "cur_param_num": (CUR_PARAM_NUM, 4),
    **{f"config_name{i + 1}": (CONFIG_NAME + PROFILE_NAME_SIZE * i, PROFILE_NAME_SIZE)
       for i in range(PROFILE_COUNT)},
    # ---- active profile
    "key_map": (KEY_MAP, 2 * KEY_COUNT),
    "key_map_fn": (KEY_MAP_FN, 2 * KEY_COUNT),
    "press_th": (0x800802C0, 2 * KEY_COUNT),
    "return_th": (0x80080370, 2 * KEY_COUNT),
    "press_deadzone": (0x80080420, 2 * KEY_COUNT),
    "return_deadzone": (0x800804D0, 2 * KEY_COUNT),
    "norm_press_th": (0x80080580, 2 * KEY_COUNT),
    "norm_return_th": (0x80080630, 2 * KEY_COUNT),
    "key_mode": (KEY_MODE, KEY_COUNT),
    "key_cancel_code": (0x80080A50, 2 * KEY_COUNT),
    "rgb": (0x80080B00, 3 * LED_COUNT),
    "rgb_solid_enable": (0x80080C08, LED_COUNT),
    "rgb_config": (0x80080C60, 5),
    "rgb_enable": (0x80080C68, 1),
    "cap_enable": (0x80080C69, 1),
    "rgb_fn_enable": (0x80080C6B, 1),
    "sleep_second_cnt": (0x80080C6C, 4),
    "win_lock_state": (0x80080C70, 1),
    "auto_calib_enable": (0x80080C71, 1),
    # ---- firmware >= 0.0.18 only
    "bind_key_mode": (BIND_KEY_MODE, KEY_COUNT),
    **{f"bind_key_setting_{k:02d}": (BIND_KEY_SETTING + BIND_RECORD_SIZE * k, BIND_RECORD_SIZE)
       for k in range(KEY_COUNT)},
}

#: Regions the firmware owns. Never write them back (a restore must skip them).
READ_ONLY = frozenset({"soft_version", "cur_param_num"})


def is_advanced(name: str) -> bool:
    """True for the regions that exist only on firmware >= ADVANCED_KEYS_MIN_VERSION."""
    return name.startswith("bind_key_")


def is_global(name: str) -> bool:
    """True for board-global regions (0x8007xxxx), which `BE C0` does not swap."""
    return REGIONS[name][0] < 0x80080000


def _merge(spans: list[tuple[int, int]]) -> tuple[tuple[int, int], ...]:
    out: list[tuple[int, int]] = []
    for start, end in sorted(spans):
        if out and start <= out[-1][1]:
            out[-1] = (out[-1][0], max(out[-1][1], end))
        else:
            out.append((start, end))
    return tuple(out)


#: Known memory as merged half-open [start, end) spans (adjacent regions are joined, so a
#: read may cross from one bind_key_setting record into the next).
SPANS = _merge([(a, a + n) for a, n in REGIONS.values()])
_READ_ONLY_SPANS = tuple((REGIONS[n][0], sum(REGIONS[n])) for n in sorted(READ_ONLY))


def in_map(addr: int, length: int) -> bool:
    """Is [addr, addr+length) completely inside known memory?

    Requests outside the map have hung the stock firmware, so the tools refuse them.
    """
    return length > 0 and any(s <= addr and addr + length <= e for s, e in SPANS)


def overlaps_read_only(addr: int, length: int) -> bool:
    """Does [addr, addr+length) touch a firmware-owned region?"""
    return any(addr < e and addr + length > s for s, e in _READ_ONLY_SPANS)
