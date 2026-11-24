#!/usr/bin/env python3
"""Phase-0 full backup of the ZENT75-HE config (read-only: BE 04 + BE C0 profile switch).
Dumps every RAM-map region for all 4 profiles into backups/<ts>/ as raw .bin + combined JSON
with sha256 sums. Restores the originally-active profile at the end. No flash/save/write.
"""
import hashlib, json, os, sys, time
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
from zentlib import Zent, wait_device

# name -> (addr, byte-length)  (from README RAM map; lengths are byte counts)
REGIONS = {
    "top_value":        (0x80070000, 166), "bot_value":        (0x800700B0, 166),
    "keytype_code":     (0x80070160, 166), "soft_version":     (0x80070210, 4),
    "cur_param_num":    (0x80070218, 4),
    "config_name1":     (0x80070290, 64),  "config_name2":     (0x800702D0, 64),
    "config_name3":     (0x80070310, 64),  "config_name4":     (0x80070350, 64),
    "key_map":          (0x80080160, 166), "key_map_fn":       (0x80080210, 166),
    "press_th":         (0x800802C0, 166), "return_th":        (0x80080370, 166),
    "press_deadzone":   (0x80080420, 166), "return_deadzone":  (0x800804D0, 166),
    "norm_press_th":    (0x80080580, 166), "norm_return_th":   (0x80080630, 166),
    "key_mode":         (0x800806E0, 83),
    "key_cancel_code":  (0x80080A50, 166),
    "rgb":              (0x80080B00, 255), "rgb_solid_enable": (0x80080C08, 85),
    "rgb_config":       (0x80080C60, 5),
    "rgb_enable":       (0x80080C68, 1),   "cap_enable":       (0x80080C69, 1),
    "rgb_fn_enable":    (0x80080C6B, 1),   "sleep_second_cnt": (0x80080C6C, 4),
    "win_lock_state":   (0x80080C70, 1),   "auto_calib_enable":(0x80080C71, 1),
    "bind_key_mode":    (0x80081DE0, 83),
}
# bind_key_setting: 48 bytes per key, 83 keys
for k in range(83):
    REGIONS[f"bind_key_setting_{k:02d}"] = (0x80080D00 + 48 * k, 48)

def dump_profile(z):
    out = {}
    for name, (addr, length) in REGIONS.items():
        out[name] = {"addr": addr, "len": length,
                     "data": z.read_ram_big(addr, length).hex()}
    return out

def main():
    dev = wait_device(20)
    if not dev:
        print("device not present"); sys.exit(1)
    z = Zent(dev)
    ver = z.version(); active = z.profile()
    print("firmware", ver, "active profile", active, "dev", dev)
    serial = z.serial()
    profiles = {}
    for p in range(4):
        z.switch_profile(p)
        got = z.profile()
        print(f"  profile {p} (active now {got}) ...", flush=True)
        profiles[p] = dump_profile(z)
    z.switch_profile(active)
    print("restored active profile", z.profile())

    ts = int(time.time())
    d = os.path.join(os.path.dirname(os.path.dirname(os.path.abspath(__file__))), "backups", str(ts))
    os.makedirs(d, exist_ok=True)
    sums = {}
    for p, regs in profiles.items():
        for name, r in regs.items():
            raw = bytes.fromhex(r["data"])
            fn = f"p{p}_{name}.bin"
            open(os.path.join(d, fn), "wb").write(raw)
            sums[fn] = hashlib.sha256(raw).hexdigest()
    meta = {"timestamp": ts, "firmware": ver, "active_profile": active,
            "serial_raw": serial.hex() if serial else None, "regions": REGIONS,
            "profiles": profiles, "sha256": sums}
    json.dump(meta, open(os.path.join(d, "dump.json"), "w"), indent=1)
    open(os.path.join(d, "SHA256SUMS"), "w").write(
        "".join(f"{v}  {k}\n" for k, v in sorted(sums.items())))
    print("backup ->", d, f"({len(sums)} regions x4 profiles)")
    z.close()

if __name__ == "__main__":
    main()
