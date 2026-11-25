#!/usr/bin/env python3
"""Generate the differential-test fixtures of tests/engine_diff.rs from the STOCK 0.0.18 engine.

Runs the stock key engine in unicorn through the git-ignored, vendor-derived harness in
re-work/engine/ (so this script only works where re-work/ exists) and writes numeric traces
only: inputs and expected outputs. Nothing vendor-derived besides numbers ends up in the repo.

    cd firmware/core/tests/fixtures
    uv run -q --with unicorn python -I gen_engine.py [scenario ...]

Fixture format (engine/<name>.txt), one record per line:

    patch b1|b2 <hex offset> <hex bytes>   config change against base_bank1.bin / base_profile.bin
    rest <v>                               every key starts at v (spare slots stay at v)
    stream <0|1>                           1: a BE FD frame every 320 scans (heartbeat forced)
    diag <0|1>                             DA flag
    watch <k> ...                          keys whose raw/travel/pressed are recorded
    noise <amp> <seed>                     uniform ±amp per key and scan (LCG, see below)
    cal <scan>                             calibration mode switched on before that scan
    seg <n> <k>=<v> <k>=<a>..<b> ...       n scans; a..b = linear ramp v0 + floor((v1-v0)*(i+1)/n)
    end
    r <scan> <key> <raw> <travel> <pressed>   watched key state after the scan (only on change)
    h <scan> <ep> <hex>                       report the stock firmware sent during that scan
    f <scan> <w0> ... <w504>                  BE FD words (1017-byte frame) built in that scan
    c <scan> <w0> ... <w497>                  calibration frame words (1003 bytes)

Noise LCG (32-bit): x = x*1103515245 + 12345; sample = ((x >> 16) & 0x7FFF) % (2*amp+1) - amp,
drawn for keys 0..82 in order every scan and added to the segment value.
"""
from __future__ import annotations
import os, struct, sys

HERE = os.path.dirname(os.path.abspath(__file__))
ROOT = os.path.abspath(os.path.join(HERE, '..', '..', '..', '..'))
RE = os.path.join(ROOT, 're-work', 'engine')
sys.path.insert(0, RE)
from harness import Engine  # noqa: E402  (vendor-derived, git-ignored)
from emu import default_flash  # noqa: E402
from unicorn import UC_HOOK_CODE  # noqa: E402

OUT = os.path.join(HERE, 'engine')
B1, B2 = 0x70000, 0x80000
B1_LEN, B2_LEN = 0x790, 0x1E38
NK = 83

# key indices
ESC, F1, BKSP, W, A, S, D, F, J, K = 0, 1, 28, 32, 46, 47, 48, 49, 52, 53
ENTER, LSHIFT, Z, C, LCTRL, LWIN, SPACE, FN, LEFT = 57, 59, 60, 62, 73, 74, 76, 78, 80
RSHIFT = 70
NAMES = dict(Esc=ESC, F1=F1, Bksp=BKSP, W=W, A=A, S=S, D=D, F=F, J=J, K=K, Enter=ENTER, LShift=LSHIFT,
             Z=Z, C=C, LCtrl=LCTRL, LWin=LWIN, Space=SPACE, FN=FN, Left=LEFT, RShift=RSHIFT)


def u16s(vals):
    return b''.join(struct.pack('<h' if v < 0 else '<H', v) for v in vals)


class Scn:
    def __init__(self, name, watch, rest=30000, stream=False, diag=False, noise=None):
        self.name, self.watch, self.rest, self.stream, self.diag, self.noise = name, watch, rest, stream, diag, noise
        self.patches = []   # (bank, off, bytes)
        self.segs = []      # (n, {k: (a, b)})
        self.cal_at = None

    # --- config
    def b1(self, off, data):
        self.patches.append(('b1', off, bytes(data))); return self

    def b2(self, off, data):
        self.patches.append(('b2', off, bytes(data))); return self

    def key16(self, base, k, v):
        return self.b2(base + 2 * k, u16s([v]))

    def key8(self, base, k, v):
        return self.b2(base + k, bytes([v]))

    def all16(self, bank, base, v):
        data = u16s([v] * 88)
        return self.b1(base, data) if bank == 1 else self.b2(base, data)

    # --- trace
    def seg(self, n, **kv):
        d = {}
        for k, v in kv.items():
            k = int(k[1:]) if k[0] == 'k' and k[1:].isdigit() else NAMES[k]
            d[k] = v if isinstance(v, tuple) else (v, v)
        self.segs.append((n, d)); return self


def run(sc: Scn):
    img = bytearray(default_flash())
    for bank, off, data in sc.patches:
        base = B1 if bank == 'b1' else B2
        img[base + off:base + off + len(data)] = data
    eng = Engine(bytes(img), rest=sc.rest)
    e = eng.e
    e.mu.hook_add(UC_HOOK_CODE, lambda uc, a, s, _: uc.mem_write(0x88242, b'\0'), begin=0xEA8E, end=0xEA8E)
    lines = [f'# stock 0.0.18 trace: {sc.name}']
    for bank, off, data in sc.patches:
        lines.append(f'patch {bank} {off:x} {data.hex()}')
    lines += [f'rest {sc.rest}', f'stream {int(sc.stream)}', f'diag {int(sc.diag)}', 'watch ' + ' '.join(map(str, sc.watch))]
    if sc.noise:
        lines.append(f'noise {sc.noise[0]} {sc.noise[1]}')
    if sc.cal_at is not None:
        lines.append(f'cal {sc.cal_at}')
    for n, d in sc.segs:
        lines.append(f'seg {n} ' + ' '.join(f'{k}={a}' if a == b else f'{k}={a}..{b}' for k, (a, b) in d.items()))
    lines.append('end')
    cur = [sc.rest] * NK
    last = {}
    x = sc.noise[1] if sc.noise else 0
    scan = 0
    for n, d in sc.segs:
        for i in range(n):
            for k, (a, b) in d.items():
                cur[k] = a + (b - a) * (i + 1) // n if a != b else a
            vals = list(cur)
            if sc.noise:
                amp = sc.noise[0]
                for k in range(NK):
                    x = (x * 1103515245 + 12345) & 0xFFFFFFFF
                    vals[k] += ((x >> 16) & 0x7FFF) % (2 * amp + 1) - amp
            if sc.cal_at is not None and scan == sc.cal_at:
                e.w32(0x91F80, 1)
            if sc.diag:
                e.w32(0x91F78, 1)
            r = eng.scan(vals, sc.stream)
            for k in sc.watch:
                t = (r['raw'][k], r['travel'][k], r['pressed'][k])
                if last.get(k) != t:
                    last[k] = t
                    lines.append(f'r {scan} {k} {t[0]} {t[1]} {t[2]}')
            for ep, b in r['reports']:
                if ep == 0x82:
                    ln = b[2] | b[3] << 8
                    w = struct.unpack('<%dH' % ((ln - 7) // 2), b[4:ln - 3])
                    lines.append(('f ' if ln == 1017 else 'c ') + f'{scan} ' + ' '.join(map(str, w)))
                else:
                    lines.append(f'h {scan} {ep:02x} {b.hex()}')
            scan += 1
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, sc.name + '.txt'), 'w') as fh:
        fh.write('\n'.join(lines) + '\n')
    print(f'{sc.name}: {scan} scans, {len(lines)} lines')


# config offsets (bank 2)
HOLD, KEY_MAP, KEY_MAP_FN = 0x000, 0x160, 0x210
PRESS_TH, RETURN_TH, PRESS_DZ, RETURN_DZ = 0x2C0, 0x370, 0x420, 0x4D0
NORM_PRESS, NORM_RETURN, KEY_MODE, MASK = 0x580, 0x630, 0x6E0, 0x790
CANCEL, AUTOCAL, BIND, BIND_MODE = 0xA50, 0xC71, 0xD00, 0x1DE0
TYPE = 0x160  # bank 1

REST, BOTTOM = 30000, 17500
SETTLE = 600  # first baseline window + margin


def press_cycle(sc, k, lo=BOTTOM, ramp=400, hold=300, gap=300):
    kk = f'k{k}'
    sc.seg(ramp, **{kk: (REST, lo)}).seg(hold, **{kk: lo}).seg(ramp, **{kk: (lo, REST)}).seg(gap, **{kk: REST})
    return sc


def dks(sc, k, actions, codes, top=500, bottom=2500, times=(8, 24, 16), b47=1):
    rec = bytearray(48)
    for i in range(4):
        for ph in range(4):
            rec[4 * i + ph] = actions[i][ph]
    rec[16:24] = u16s(codes)
    rec[24:26] = u16s([top]); rec[26:28] = u16s([bottom])
    rec[28:34] = u16s(list(times))
    rec[34:47] = b'\xff' * 13
    rec[47] = b47
    sc.b2(BIND + 48 * k, rec).key8(BIND_MODE, k, 2 if actions is not None else 1)
    return sc


def scenarios():
    out = []
    # ---- signal path
    s = Scn('rest_noise_drift', [F], stream=True, noise=(25, 7))
    s.seg(SETTLE).seg(9000, F=(REST, REST + 30), J=(REST, REST - 14), Space=(REST, REST + 9)).seg(3000)
    out.append(s)
    for t in range(4):
        s = Scn(f'press_type{t}', [F, J], stream=True).all16(1, TYPE, t)
        s.seg(SETTLE)
        press_cycle(s, F, ramp=1500, hold=200, gap=200)       # slow
        press_cycle(s, J, ramp=12, hold=200, gap=200)         # fast
        press_cycle(s, F, lo=24000, ramp=40, hold=100, gap=100)  # partial
        out.append(s)
    s = Scn('press_custom', [F, J], stream=True).all16(1, TYPE, 4095)   # custom record 0 (type 4)
    press_cycle(s.seg(SETTLE), F, ramp=300)
    out.append(s)
    s = Scn('mixed_types', [F, J, K, SPACE], stream=True).b1(TYPE, u16s([k % 4 for k in range(88)]))
    s.seg(SETTLE)
    s.seg(200, F=(REST, 18000), J=(REST, 19000)).seg(300, K=(REST, 17500), Space=(REST, 22000)).seg(200)
    s.seg(200, F=(18000, REST), J=(19000, REST), K=(17500, REST), Space=(22000, REST)).seg(600)
    out.append(s)
    # ---- actuation
    s = Scn('normal_hysteresis', [F]).seg(SETTLE)
    s.seg(300, F=(REST, 25500))
    for _ in range(4):
        s.seg(60, F=(25500, 26200)).seg(60, F=(26200, 25200))
    s.seg(300, F=(25200, 28500)).seg(100, F=(28500, 27000)).seg(300, F=(27000, REST)).seg(100)
    out.append(s)
    for name, pdz, rdz, pt, rt in (('rt_dz200', 200, 200, 100, 100), ('rt_dz0', 0, 0, 10, 30), ('rt_factory', 55, 0, 50, 30)):
        s = Scn(name, [F, A]).key8(KEY_MODE, F, 1).key16(PRESS_DZ, F, pdz).key16(RETURN_DZ, F, rdz)
        s.key16(PRESS_TH, F, pt).key16(RETURN_TH, F, rt).seg(SETTLE)
        s.seg(400, F=(REST, 22000)).seg(80, F=(22000, 23500)).seg(80, F=(23500, 21000))
        s.seg(60, F=(21000, 18000)).seg(200, F=17500).seg(60, F=(17500, 19500)).seg(60, F=(19500, 17400))
        s.seg(300, F=(17400, 28000)).seg(100, F=(28000, 26000)).seg(300, F=(26000, REST)).seg(200)
        s.seg(30, A=(REST, 20000)).seg(200).seg(30, A=(20000, REST)).seg(100)   # factory RT key, fast
        out.append(s)
    for name, k, rt in (('mode2', F, 100), ('mode2_space_guard', SPACE, 100)):
        s = Scn(name, [k]).key8(KEY_MODE, k, 2).key16(RETURN_TH, k, rt).key16(PRESS_TH, k, 100).seg(SETTLE)
        kk = f'k{k}'
        s.seg(300, **{kk: (REST, 20000)}).seg(100, **{kk: (20000, 22500)}).seg(100, **{kk: (22500, 19000)})
        s.seg(100, **{kk: (19000, 17300)}).seg(300, **{kk: 17300}).seg(100, **{kk: (17300, 21000)})
        s.seg(100, **{kk: (21000, 18000)}).seg(300, **{kk: (18000, REST)}).seg(200)
        out.append(s)
    s = Scn('rt_space_guard', [SPACE]).key8(KEY_MODE, SPACE, 1).key16(RETURN_TH, SPACE, 100).seg(SETTLE)
    s.seg(40, Space=(REST, 17300)).seg(300).seg(20, Space=(17300, 18500)).seg(10, Space=(18500, 17300)).seg(300)
    s.seg(20, Space=(17300, 29900)).seg(30, Space=(29900, 26000)).seg(300, Space=(26000, REST)).seg(200)
    out.append(s)
    # ---- SOCD
    s = Scn('socd_pair', [A, D]).key8(KEY_MODE, A, 0).key8(KEY_MODE, D, 0)
    s.key16(CANCEL, A, 7).key16(CANCEL, D, 4).seg(SETTLE)
    s.seg(30, A=(REST, 20000)).seg(200).seg(30, D=(REST, 20000)).seg(200).seg(30, D=(20000, REST)).seg(200)
    s.seg(30, D=(REST, 20000)).seg(200).seg(30, A=(20000, REST)).seg(200).seg(30, D=(20000, REST)).seg(200)
    out.append(s)
    s = Scn('socd_single', [A, D]).key8(KEY_MODE, A, 0).key8(KEY_MODE, D, 0).key16(CANCEL, A, 7).seg(SETTLE)
    s.seg(30, A=(REST, 20000)).seg(200).seg(30, D=(REST, 20000)).seg(200).seg(30, D=(20000, REST)).seg(200)
    s.seg(30, D=(REST, 20000)).seg(200).seg(30, A=(20000, REST)).seg(200).seg(30, A=(REST, 20000)).seg(200)
    s.seg(30, A=(20000, REST), D=(20000, REST)).seg(200)
    out.append(s)
    s = Scn('socd_modifier', [A, LSHIFT]).key8(KEY_MODE, A, 0).key16(CANCEL, A, -7).key16(CANCEL, F, 201).seg(SETTLE)
    s.seg(30, LShift=(REST, 20000)).seg(100).seg(30, A=(REST, 20000)).seg(100).seg(30, A=(20000, REST)).seg(100)
    s.seg(30, F=(REST, 20000)).seg(100).seg(30, F=(20000, REST), LShift=(20000, REST)).seg(100)
    out.append(s)
    # ---- DKS / groups
    for name, acts in (
        ('dks_hold_release', [[1, 0, 0, 0], [0, 1, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]]),
        ('dks_taps', [[2, 2, 2, 2], [0, 1, 2, 0], [1, 0, 0, 2], [0, 0, 0, 0]]),
    ):
        s = Scn(name, [F])
        dks(s, F, acts, [4, 5, 6, 1000], times=(8, 24, 200))
        s.seg(SETTLE)
        for lo, r in ((17500, 300), (24000, 200), (17500, 20)):
            s.seg(r, F=(REST, lo)).seg(300).seg(80, F=(lo, 26000)).seg(80, F=(26000, lo)).seg(r, F=(lo, REST)).seg(300)
        out.append(s)
    s = Scn('dks_trigger_original', [F])
    dks(s, F, [[2, 0, 2, 0], [1, 0, 0, 0], [0, 0, 0, 0], [0, 0, 0, 0]], [4, 5, 1000, 1000], b47=0)
    press_cycle(s.seg(SETTLE), F, ramp=200)
    out.append(s)
    s = Scn('key_group', [F])
    rec = bytearray(48); rec[16:24] = u16s([4, -7, 1000, 225]); rec[24:34] = u16s([100, 3000, 8, 24, 16]); rec[47] = 0
    s.b2(BIND + 48 * F, rec).key8(BIND_MODE, F, 1)
    press_cycle(s.seg(SETTLE), F, ramp=100)
    out.append(s)
    # ---- tap-hold, masks
    s = Scn('tap_hold', [F, J]).key16(HOLD, F, -8).key16(HOLD, J, 5).key8(MASK, K, 0x22).seg(SETTLE)
    s.seg(20, F=(REST, 20000)).seg(300).seg(20, F=(20000, REST)).seg(100)          # tap
    s.seg(20, F=(REST, 20000)).seg(4500).seg(20, F=(20000, REST)).seg(100)         # hold
    s.seg(20, J=(REST, 20000)).seg(4200).seg(20, J=(20000, REST)).seg(100)
    s.seg(20, K=(REST, 20000)).seg(100).seg(20, K=(20000, REST)).seg(100)
    out.append(s)
    # ---- layers
    s = Scn('fn_layer', [FN, F, J]).key16(KEY_MAP_FN, F, 30).key16(KEY_MAP_FN, J, 1000).seg(SETTLE)
    s.seg(30, FN=(REST, 20000)).seg(100).seg(30, F=(REST, 20000), J=(REST, 20000)).seg(200)
    s.seg(30, FN=(20000, REST)).seg(200).seg(30, F=(20000, REST), J=(20000, REST)).seg(100)
    s.seg(30, F=(REST, 20000)).seg(100).seg(30, FN=(REST, 20000)).seg(100).seg(30, F=(20000, REST), FN=(20000, REST)).seg(100)
    out.append(s)
    # ---- function keys
    s = Scn('fn_profile_switch', [FN, C, F]).seg(SETTLE)
    s.seg(30, FN=(REST, 20000)).seg(50).seg(30, C=(REST, 20000)).seg(100).seg(30, C=(20000, REST))
    s.seg(30, F=(REST, 20000)).seg(200).seg(30, F=(20000, REST)).seg(200)
    s.seg(30, C=(REST, 20000)).seg(100).seg(30, C=(20000, REST)).seg(30, FN=(20000, REST))
    s.seg(8000).seg(30, F=(REST, 20000)).seg(100).seg(30, F=(20000, REST)).seg(100)
    out.append(s)
    s = Scn('fn_winlock', [FN, LWIN]).seg(SETTLE)
    s.seg(30, LWin=(REST, 20000)).seg(100).seg(30, LWin=(20000, REST)).seg(50)
    s.seg(30, FN=(REST, 20000)).seg(50).seg(30, LWin=(REST, 20000)).seg(100).seg(30, LWin=(20000, REST)).seg(30, FN=(20000, REST)).seg(50)
    s.seg(30, LWin=(REST, 20000)).seg(100).seg(30, LShift=(REST, 20000)).seg(100).seg(30, LWin=(20000, REST), LShift=(20000, REST)).seg(50)
    out.append(s)
    s = Scn('func_codes', [F, J, K]).key16(KEY_MAP, F, 306).key16(KEY_MAP, J, 320).key16(KEY_MAP, K, 203)
    s.key16(KEY_MAP, 54, 251).key16(KEY_MAP, 55, 225).key16(KEY_MAP, 56, 315).seg(SETTLE)
    s.seg(30, F=(REST, 20000)).seg(100).seg(30, F=(20000, REST)).seg(50)
    s.seg(30, LShift=(REST, 20000)).seg(50).seg(30, F=(REST, 20000)).seg(100).seg(30, F=(20000, REST), LShift=(20000, REST)).seg(50)
    s.seg(30, J=(REST, 20000)).seg(100).seg(30, J=(20000, REST)).seg(50)
    s.seg(30, A=(REST, 20000), Left=(REST, 20000)).seg(100).seg(30, A=(20000, REST), Left=(20000, REST)).seg(50)
    s.seg(30, K=(REST, 20000), k54=(REST, 20000), k55=(REST, 20000), k56=(REST, 20000)).seg(100)
    s.seg(30, K=(20000, REST), k54=(20000, REST), k55=(20000, REST), k56=(20000, REST)).seg(50)
    out.append(s)
    # ---- rollover
    s = Scn('rollover', [F])
    keys = list(range(16, 28)) + list(range(31, 44))   # 25 keys
    s.seg(SETTLE)
    for i, k in enumerate(keys):
        s.seg(25, **{f'k{k}': (REST, 20000)})
    s.seg(100)
    for i, k in enumerate(keys):
        s.seg(25, **{f'k{k}': (20000, REST)})
        s.seg(5)
    s.seg(100).seg(30, F=(REST, 20000)).seg(100).seg(30, F=(20000, REST)).seg(100)
    out.append(s)
    # ---- calibration mode, DA, auto-cal
    s = Scn('calibration_frames', [F], noise=(15, 3))
    s.seg(SETTLE + 100).seg(8200, F=(REST, 25000)).seg(400)
    s.cal_at = SETTLE + 50
    out.append(s)
    s = Scn('da_arrays', [F, J], stream=True, diag=True).seg(SETTLE)
    press_cycle(s, F, ramp=300, hold=300, gap=300)
    press_cycle(s, J, lo=26000, ramp=200, hold=100, gap=300)
    out.append(s)
    s = Scn('autocal', [F], stream=True).seg(SETTLE)
    for _ in range(3):
        s.seg(25, F=(REST, 16040)).seg(6000).seg(25, F=(16040, REST)).seg(700)
    s.seg(200, F=(REST, 23000)).seg(100).seg(200, F=(23000, REST)).seg(200)
    out.append(s)
    s = Scn('autocal_off', [F], stream=True).b2(AUTOCAL, [0]).seg(SETTLE)
    s.seg(25, F=(REST, 16040)).seg(6000).seg(25, F=(16040, REST)).seg(700)
    out.append(s)
    # ---- held key at power-up (stock: stuck at travel 0)
    s = Scn('held_at_boot', [F, J], stream=True).seg(1200, F=20000).seg(30, F=(20000, REST))
    s.seg(3000).seg(30, F=(REST, 20000)).seg(200).seg(30, F=(20000, REST)).seg(300)
    out.append(s)
    return out


def main():
    want = set(sys.argv[1:])
    os.makedirs(OUT, exist_ok=True)
    img = default_flash()
    open(os.path.join(OUT, 'base_bank1.bin'), 'wb').write(img[B1:B1 + B1_LEN])
    open(os.path.join(OUT, 'base_profile.bin'), 'wb').write(img[B2:B2 + B2_LEN])
    for sc in scenarios():
        if not want or sc.name in want:
            run(sc)


if __name__ == '__main__':
    main()
