#!/usr/bin/env python3
"""Regenerate the stock-firmware reference fixtures used by the core crate's differential tests.

Runs only where the git-ignored RE scratch (`re-work/config/`: emulator, harness, vendor image)
exists. Everything written here is numeric output of the stock firmware (per-LED RGB frames,
config images it writes as defaults or after repair), not code.

    cd firmware/core/tests/fixtures
    uv run -q --with unicorn python -I gen_rgb.py            # all
    uv run -q --with unicorn python -I gen_rgb.py rgb        # only the lighting frames
    uv run -q --with unicorn python -I gen_rgb.py config     # only defaults / repair vectors

Outputs:
  rgb/<case>.txt         header line `# key=value ...` (harness options), then one line per
                         frame: `<tick> <255 bytes hex>` (R,G,B per LED as handed to the WS2812
                         encoder, i.e. after effect + overlays + rgb_enable).
  config/bank1.bin       factory bank 1 (0x790 bytes) as stock creates it on blank NOR
  config/profileN.bin    factory profile N (0x1E38 bytes)
  config/repair/*.in|out crafted bank images and what stock's boot validation/repair makes of them
"""
import contextlib, io, os, random, struct, sys
from argparse import Namespace

HERE = os.path.dirname(os.path.abspath(__file__))
RE = os.path.normpath(os.path.join(HERE, '../../../../re-work/config'))
sys.path.insert(0, RE)
os.chdir(RE)
import rgb_harness  # noqa: E402
from emu import Emu, default_images  # noqa: E402


def harness(effect, **kw):
    a = Namespace(effect=effect, frames=6, h=0x90, s=0xff, bright=10, speed=0x32, flags=0xff, t0=0,
                  step=8, press=[], height=0, static=[], caps=False, fn=False, off=False,
                  config='blank', raw=True)
    for k, v in kw.items():
        setattr(a, k, v)
    frames = []
    with contextlib.redirect_stdout(io.StringIO()):
        for fr in rgb_harness.render(a):
            frames.append(bytes(fr))
    return a, frames


def write_case(name, effect, **kw):
    a, frames = harness(effect, **kw)
    os.makedirs(os.path.join(HERE, 'rgb'), exist_ok=True)
    hdr = dict(effect=a.effect, h=a.h, s=a.s, bright=a.bright, speed=a.speed, flags=a.flags, t0=a.t0,
               step=a.step, height=a.height)
    parts = [f'{k}={v}' for k, v in hdr.items()]
    parts += [f'press={p}' for p in a.press] + [f'static={s}' for s in a.static]
    parts += [f for f in ('caps', 'fn', 'off') if getattr(a, f)]
    with open(os.path.join(HERE, 'rgb', name + '.txt'), 'w') as f:
        f.write('# ' + ' '.join(parts) + '\n')
        for i, fr in enumerate(frames):
            f.write(f'{a.t0 + i * a.step} {fr.hex()}\n')


STATEFUL = {24, 25, 29, 30, 31, 32, 33, 34, 47, 48, 49}
REACTIVE = {32, 35, 36, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46}


def gen_rgb():
    for e in range(0, 53):
        # A: factory colours, irregular steps so time-based effects are sampled at many phases
        write_case(f'e{e:02}_a', e, frames=8 if e not in STATEFUL else 40, t0=1000,
                   step=37 if e not in STATEFUL else 8)
        # B: other colour / max brightness / fast, timer past the 16-bit wrap
        write_case(f'e{e:02}_b', e, frames=6 if e not in STATEFUL else 30, h=0x10, s=0xa0, bright=30,
                   speed=200, t0=70000, step=8 if e in STATEFUL else 53)
    for e in sorted(REACTIVE):
        write_case(f'e{e:02}_press', e, frames=24, step=8, t0=8,
                   press=['46@1-3', '20@6-7', '76@9-9', '45@12-20'])
    write_case('e33_depth', 33, frames=6, step=8, t0=8, press=['46@0-5', '50@2-5'], height=1500)
    write_case('e01_static', 1, frames=2, static=['0=ff0000', '10=00ff00', '40=0000ff', '84=ffffff',
                                                     '50=123456'])
    write_case('e13_static', 13, frames=3, step=100, static=['3=ff8000', '4=ff8000', '5=ff8000'])
    write_case('e13_static_b30', 13, frames=2, bright=30, static=['20=ffffff', '21=808080', '22=010203'])
    write_case('e05_caps', 5, frames=3, step=50, caps=True)
    write_case('e05_fn', 5, frames=3, step=50, fn=True)
    write_case('e05_fn_caps', 5, frames=2, step=50, fn=True, caps=True)
    write_case('e13_off', 13, frames=2, off=True)
    write_case('e13_flags04', 13, frames=2, flags=0x04)
    write_case('e13_flags02', 13, frames=2, flags=0x02)
    write_case('e01_bright40', 1, frames=1, bright=40)
    # long runs of the random / stateful effects so their state machines get exercised
    write_case('e24_long', 24, frames=80, speed=200, step=13, t0=5)
    write_case('e25_long', 25, frames=80, speed=200, step=13, t0=5)
    write_case('e29_long', 29, frames=60, speed=255, step=20)
    write_case('e30_long', 30, frames=60, speed=255, step=50)
    write_case('e31_long', 31, frames=60, speed=255, step=50)
    write_case('e34_long', 34, frames=150, bright=30)
    write_case('e34_dim', 34, frames=70, bright=1)
    write_case('e34_zero', 34, frames=35, bright=0)
    for e in (47, 48, 49):
        write_case(f'e{e}_long', e, frames=60, speed=100, step=11)
    write_case('e32_long', 32, frames=60, step=30, t0=8, press=['46@1-30', '47@3-3', '20@10-12', '76@20-21'])
    write_case('e24_caps', 24, frames=20, step=8, caps=True, speed=255)


def gen_config():
    out = os.path.join(HERE, 'config')
    os.makedirs(os.path.join(out, 'repair'), exist_ok=True)
    b1, profs = default_images()
    open(os.path.join(out, 'bank1.bin'), 'wb').write(b1)
    for p in range(4):
        open(os.path.join(out, f'profile{p}.bin'), 'wb').write(profs[p])
    rnd = random.Random(18)

    def prof_case(name, p, img):
        img = bytes(img)
        assert len(img) == 0x1E38
        e = Emu(flash={0x80080000 + 0x2000 * p: img})
        e.w32(0x729c, p)
        e.call(0x1385c)
        open(os.path.join(out, 'repair', f'p{p}_{name}.in'), 'wb').write(img)
        open(os.path.join(out, 'repair', f'p{p}_{name}.out'), 'wb').write(e.rd(0x658, 0x1E38))

    def fld(img, off, data):
        img = bytearray(img)
        img[off:off + len(data)] = data
        return img

    for p in range(4):
        prof_case('blank', p, b'\xff' * 0x1E38)
        prof_case('random', p, bytes(rnd.randrange(256) for _ in range(0x1E38)))
    d0 = profs[0]
    u16 = lambda v: struct.pack('<H', v)
    cases = {
        'valid_edits': fld(fld(fld(d0, 0x160, u16(5)), 0xB00, b'\x12\x34\x56'), 0xC60, b'\x51\x11\x22\x1f\x99\x04\x00\x07'),
        'keymap_ff': fld(d0, 0x160, b'\xff' * 0xB0),
        'keymap_fn_ff': fld(d0, 0x210, b'\xff' * 0xB0),
        'press_th_8000': fld(d0, 0x2C0, u16(0x8000)),
        'press_th_5001': fld(d0, 0x2C0, u16(5001)),
        'ret_dz_ffff': fld(d0, 0x4D0, u16(0xFFFF)),
        'norm_ret_5000': fld(d0, 0x630, u16(5000)),
        'key_mode_11': fld(d0, 0x6E0, b'\x0b'),
        'layer_a_1001': fld(d0, 0x000, u16(1001)),
        'layer_a_ffff': fld(d0, 0x000, u16(0xFFFF)),
        'layer_a_all_ff': fld(d0, 0x000, b'\xff' * 0xB0),
        'sleep_3601': fld(d0, 0xC6C, struct.pack('<I', 3601)),
        'sleep_neg': fld(d0, 0xC6C, struct.pack('<I', 0xFFFFFFFF)),
        'rgbcfg_ff': fld(d0, 0xC60, b'\xff' * 4),
        'cancel_all_ff': fld(d0, 0xA50, b'\xff' * 0xB0),
        'c70_11': fld(d0, 0xC70, b'\x0b'),
        'c71_ff': fld(d0, 0xC71, b'\xff'),
        'bind_all_ff': fld(fld(d0, 0xD00, b'\xff' * 0x1080), 0x1DE0, b'\xff' * 0x58),
        'bind_top_ffff': fld(d0, 0xD00 + 0x18, u16(0xFFFF)),
        'bind_mode_3': fld(d0, 0x1DE0, b'\x03' * 0x58),
        'bright_31': fld(d0, 0xC63, b'\x1f'),
        'gaps_kept': fld(fld(fld(d0, 0xBFF, b'\x01' * 9), 0xC66, b'\x02\x03'), 0xC72, b'\x04' * 0x8E),
    }
    for name, img in cases.items():
        for p in (0, 2):
            prof_case(name, p, img)

    def b1_case(name, img):
        img = bytes(img)
        e = Emu(flash={0x80070000: img})
        open(os.path.join(out, 'repair', f'b1_{name}.in'), 'wb').write(img)
        open(os.path.join(out, 'repair', f'b1_{name}.out'), 'wb').write(e.rd(0x7084, 0x790))

    b1c = {
        'blank': b'\xff' * 0x790,
        # cur_param_num kept in range: stock compares it signed, a negative value would make it
        # read a profile from a wild NOR address (we treat anything >= 4 as 0)
        'random': fld(fld(bytes(rnd.randrange(256) for _ in range(0x790)), 0x218, struct.pack('<I', 1)),
                      0x160, struct.pack('<H', 2) * 88),  # random switch types crash stock's key setup
        'valid_p2': fld(fld(b1, 0x218, struct.pack('<I', 2)), 0x000, u16(29000)),
        'cur_4': fld(b1, 0x218, struct.pack('<I', 4)),
        'ver_13': fld(b1, 0x210, struct.pack('<I', 0x13)),
        'w214_0': fld(b1, 0x214, struct.pack('<I', 0)),
        'w214_1': fld(b1, 0x214, struct.pack('<I', 1)),
        'keytype_ff': fld(b1, 0x160, u16(0xFFFF)),
        'top_ff': fld(b1, 0x000, u16(0xFFFF)),
        'bot_ff': fld(b1, 0x0B0, u16(0xFFFF)),
        'name2_ff': fld(b1, 0x290 + 64 * 2, b'\xff'),
        'cs1_travel': fld(b1, 0x390 + 256 * 1, struct.pack('<I', 1499)),
        'cs2_n89': fld(b1, 0x390 + 256 * 2 + 4, struct.pack('<I', 89)),
        'cs3_last': fld(b1, 0x390 + 256 * 3 + 8 + 2 * 70, u16(2637)),
        'cs0_decr': fld(b1, 0x390 + 8 + 2 * 10, u16(0)),
        'cs0_short': fld(fld(b1, 0x390 + 4, struct.pack('<I', 3)), 0x390 + 8, b'\x00\x00\x10\x00\x4c\x0a'),
        'calib_owner_like': fld(fld(b1, 0x000, u16(30500) * 88), 0x0B0, u16(18200) * 88),
    }
    for name, img in b1c.items():
        b1_case(name, img)


if __name__ == '__main__':
    what = sys.argv[1:] or ['rgb', 'config']
    if 'config' in what:
        gen_config()
    if 'rgb' in what:
        gen_rgb()
