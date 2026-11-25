//! Differential tests: replay traces recorded from the stock 0.0.18 engine running in unicorn
//! (`tests/fixtures/gen_engine.py`) through our engine + key pipeline with `Compat::STOCK` and
//! require exact equality of raw / travel / pressed of the watched keys after every scan, of
//! every HID report the stock firmware sent (scan, endpoint, bytes), of the BE FD frames
//! (words 0..=498; the tail statistics come from the platform) and of the calibration frames.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use zent_core::config::{Global, Profile};
use zent_core::engine::compat::Compat;
use zent_core::engine::tables::OBJ_TO_SLOT;
use zent_core::engine::Engine;
use zent_core::keys::KeyProcessor;
use zent_core::layout::SLOT_COUNT;
use zent_core::protocol::REPORT_SIZE;

const DIR: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/fixtures/engine");

#[derive(Default)]
struct Expect {
    r: Vec<(usize, u16, u16, u8)>,
    h: Vec<(u8, Vec<u8>)>,
    f: Option<Vec<u16>>,
    c: Option<Vec<u16>>,
}

struct Fixture {
    g: Global,
    p: Profile,
    rest: i32,
    stream: bool,
    diag: bool,
    watch: Vec<usize>,
    noise: Option<(i32, u32)>,
    cal: Option<usize>,
    segs: Vec<(usize, Vec<(usize, i32, i32)>)>,
    exp: BTreeMap<usize, Expect>,
}

fn hex(s: &str) -> Vec<u8> {
    (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
}

fn load(name: &str) -> Fixture {
    let b1 = std::fs::read(format!("{DIR}/base_bank1.bin")).unwrap();
    let b2 = std::fs::read(format!("{DIR}/base_profile.bin")).unwrap();
    let mut g = Global::zeroed();
    let n = g.0.len().min(b1.len());
    g.0[..n].copy_from_slice(&b1[..n]);
    let mut p = Profile::zeroed();
    let n = p.0.len().min(b2.len());
    p.0[..n].copy_from_slice(&b2[..n]);
    let text = std::fs::read_to_string(format!("{DIR}/{name}.txt")).unwrap_or_else(|e| panic!("{name}: {e}"));
    let mut fx = Fixture { g, p, rest: 30000, stream: false, diag: false, watch: vec![], noise: None, cal: None, segs: vec![], exp: BTreeMap::new() };
    for line in text.lines() {
        let w: Vec<&str> = line.split_whitespace().collect();
        if w.is_empty() || w[0].starts_with('#') {
            continue;
        }
        let num = |i: usize| w[i].parse::<i64>().unwrap();
        match w[0] {
            "patch" => {
                let off = usize::from_str_radix(w[2], 16).unwrap();
                let d = hex(w[3]);
                let bank: &mut [u8] = if w[1] == "b1" { &mut fx.g.0 } else { &mut fx.p.0 };
                for (i, b) in d.iter().enumerate() {
                    if let Some(x) = bank.get_mut(off + i) {
                        *x = *b;
                    }
                }
            }
            "rest" => fx.rest = num(1) as i32,
            "stream" => fx.stream = num(1) != 0,
            "diag" => fx.diag = num(1) != 0,
            "watch" => fx.watch = w[1..].iter().map(|s| s.parse().unwrap()).collect(),
            "noise" => fx.noise = Some((num(1) as i32, num(2) as u32)),
            "cal" => fx.cal = Some(num(1) as usize),
            "seg" => {
                let kv = w[2..]
                    .iter()
                    .map(|s| {
                        let (k, v) = s.split_once('=').unwrap();
                        let (a, b) = v.split_once("..").unwrap_or((v, v));
                        (k.parse().unwrap(), a.parse().unwrap(), b.parse().unwrap())
                    })
                    .collect();
                fx.segs.push((num(1) as usize, kv));
            }
            "end" => {}
            "r" => fx.exp.entry(num(1) as usize).or_default().r.push((num(2) as usize, num(3) as u16, num(4) as u16, num(5) as u8)),
            "h" => fx.exp.entry(num(1) as usize).or_default().h.push((u8::from_str_radix(w[2], 16).unwrap(), hex(w[3]))),
            "f" => fx.exp.entry(num(1) as usize).or_default().f = Some(w[2..].iter().map(|s| s.parse().unwrap()).collect()),
            "c" => fx.exp.entry(num(1) as usize).or_default().c = Some(w[2..].iter().map(|s| s.parse().unwrap()).collect()),
            x => panic!("{name}: unknown record {x}"),
        }
    }
    fx
}

fn words(out: &[u8], n: usize) -> Vec<u16> {
    (0..n).map(|i| u16::from_le_bytes([out[4 + 2 * i], out[5 + 2 * i]])).collect()
}

/// Replay a fixture; returns the mismatches (empty = exact).
fn replay(name: &str, compat: Compat) -> Vec<String> {
    let fx = load(name);
    let mut e = Engine::new();
    let mut k = KeyProcessor::new();
    e.set_compat(compat);
    k.set_compat(compat);
    e.configure(&fx.g, &fx.p);
    k.configure(&fx.p);
    e.init_samples(&[fx.rest as u16; SLOT_COUNT]); // the harness powers up at rest
    e.skip_warmup();
    let mut cur = [fx.rest; 83];
    let mut lcg = fx.noise.map(|n| n.1).unwrap_or(0);
    let mut state: BTreeMap<usize, (u16, u16, u8)> = BTreeMap::new();
    let mut bad = Vec::new();
    let mut out = [0u8; REPORT_SIZE];
    let mut scan = 0usize;
    let mut cal = false;
    for (n, kv) in &fx.segs {
        for i in 0..*n {
            for &(key, a, b) in kv {
                cur[key] = if a != b { a + ((b - a) * (i as i32 + 1)).div_euclid(*n as i32) } else { a };
            }
            let mut slots = [fx.rest as u16; SLOT_COUNT];
            for key in 0..83 {
                let mut v = cur[key];
                if let Some((amp, _)) = fx.noise {
                    lcg = lcg.wrapping_mul(1103515245).wrapping_add(12345);
                    v += ((lcg >> 16) & 0x7FFF) as i32 % (2 * amp + 1) - amp;
                }
                slots[OBJ_TO_SLOT[key] as usize] = v as u16;
            }
            if fx.cal == Some(scan) {
                cal = true;
                e.set_calibration(true);
            }
            if fx.diag {
                e.set_diag(true);
            }
            e.scan(&slots, &fx.g, &fx.p, cal);
            let _ = k.update(&e, &fx.p, &fx.g);
            let ex = fx.exp.get(&scan);
            if let Some(ex) = ex {
                for &(key, raw, travel, pressed) in &ex.r {
                    state.insert(key, (raw, travel, pressed));
                }
            }
            for &key in &fx.watch {
                let ours = (e.raw[key], e.travel[key], k.object(key).pressed);
                if let Some(&exp) = state.get(&key)
                    && ours != exp
                {
                    bad.push(format!("scan {scan} key {key}: raw/travel/pressed ours {ours:?} stock {exp:?}"));
                }
            }
            let sent: Vec<(u8, Vec<u8>)> = k.sent().iter().map(|s| (s.ep, s.data[..s.len as usize].to_vec())).collect();
            let exp_h = ex.map(|x| x.h.clone()).unwrap_or_default();
            if sent != exp_h {
                let f = |v: &[(u8, Vec<u8>)]| v.iter().fold(String::new(), |mut s, (ep, d)| {
                    let _ = write!(s, " {ep:02x}:{}", d.iter().map(|b| format!("{b:02x}")).collect::<String>());
                    s
                });
                bad.push(format!("scan {scan}: reports ours [{}] stock [{}]", f(&sent), f(&exp_h)));
            }
            if fx.stream
                && let Some(fw) = ex.and_then(|x| x.f.as_ref())
            {
                let n = e.stream_frame(&mut out, fx.diag);
                assert_eq!(n, 1017);
                let ours = words(&out, 505);
                for (i, (a, b)) in ours.iter().zip(fw).enumerate().take(499) {
                    if a != b {
                        bad.push(format!("scan {scan}: BE FD word {i} ours {a} stock {b}"));
                    }
                }
            }
            let cf = e.take_calibration_frame(&mut out).map(|n| {
                assert_eq!(n, 1003);
                words(&out, 498)
            });
            let exp_c = ex.and_then(|x| x.c.clone());
            if cf != exp_c {
                bad.push(format!("scan {scan}: calibration frame ours {:?} stock {:?}", cf.as_ref().map(|v| &v[..6]), exp_c.as_ref().map(|v| &v[..6])));
            }
            scan += 1;
        }
    }
    bad
}

fn check(name: &str) {
    let bad = replay(name, Compat::STOCK);
    if !bad.is_empty() {
        panic!("{name}: {} mismatches, first:\n{}", bad.len(), bad.iter().take(12).cloned().collect::<Vec<_>>().join("\n"));
    }
}

macro_rules! diff {
    ($($name:ident),* $(,)?) => {
        $( #[test] fn $name() { check(stringify!($name)); } )*
    };
}

diff!(
    rest_noise_drift,
    press_type0,
    press_type1,
    press_type2,
    press_type3,
    normal_hysteresis,
    rt_dz200,
    rt_dz0,
    rt_factory,
    mode2,
    mode2_space_guard,
    rt_space_guard,
    socd_pair,
    socd_single,
    socd_modifier,
    dks_hold_release,
    dks_taps,
    dks_trigger_original,
    key_group,
    tap_hold,
    fn_layer,
    fn_profile_switch,
    fn_winlock,
    func_codes,
    rollover,
    calibration_frames,
    da_arrays,
    autocal,
    autocal_off,
    held_at_boot,
);
