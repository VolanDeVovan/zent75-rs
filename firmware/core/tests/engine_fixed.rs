//! The fixed behaviour (`Compat::FIXED`, what the firmware runs) against the stock traces:
//! everything that is not a deliberate fix must stay equivalent, and each fix must show.

mod common;

use common::{load, replay_with};
use zent_core::config::Profile;
use zent_core::engine::compat::Compat;
use zent_core::engine::tables::OBJ_TO_SLOT;
use zent_core::engine::{Engine, KeyPass};
use zent_core::keys::{Event, KeyProcessor, LightCmd};
use zent_core::layout::SLOT_COUNT;

/// Keyboard/consumer/mouse report sequences, consecutive duplicates removed.
fn seq(v: &[(usize, u8, Vec<u8>)]) -> Vec<(usize, u8, Vec<u8>)> {
    let mut out: Vec<(usize, u8, Vec<u8>)> = Vec::new();
    for (s, ep, d) in v {
        if out.iter().rev().find(|x| x.1 == *ep).is_some_and(|x| &x.2 == d) {
            continue;
        }
        out.push((*s, *ep, d.clone()));
    }
    out
}

/// FIXED sends the same reports as stock, each within `tol` scans.
fn equivalent(name: &str, tol: usize) {
    let (_, ours, stock) = replay_with(name, Compat::FIXED, |_, _, _| {});
    let (a, b) = (seq(&ours), seq(&stock));
    let strip = |v: &[(usize, u8, Vec<u8>)]| v.iter().map(|x| (x.1, x.2.clone())).collect::<Vec<_>>();
    assert_eq!(strip(&a), strip(&b), "{name}: report sequence differs");
    for (x, y) in a.iter().zip(&b) {
        assert!(x.0.abs_diff(y.0) <= tol, "{name}: report {:?} at scan {} vs stock {}", x.2, x.0, y.0);
    }
}

macro_rules! equiv {
    ($($name:ident),* $(,)?) => {
        mod equivalent {
            $( #[test] fn $name() { super::equivalent(stringify!($name), 2); } )*
        }
    };
}

equiv!(
    press_type0, press_type1, press_type2, press_type3, press_custom, mixed_types, normal_hysteresis, rt_dz200,
    rt_dz0, rt_factory, mode2, mode2_space_guard, rt_space_guard, socd_pair, socd_single, socd_modifier,
    dks_hold_release, dks_taps, dks_trigger_original, key_group, tap_hold, fn_profile_switch, fn_winlock,
    rollover, da_arrays, autocal_off,
);
// Not in the list: `fn_layer` and `func_codes` release two keys (Fn + F, LShift + `~ESC`) in
// the same scan. Which one the firmware sees first depends on the evaluation step, which the
// step fix changes, so stock shows a one-step intermediate report (F on the main layer, plain
// Esc) that the fixed order does not, or vice versa. Everything else in them is identical.

#[test]
fn held_key_at_power_up_recovers() {
    // stock: F held through the baseline capture reads travel 0 forever and never actuates
    let (_, _, stock) = replay_with("held_at_boot", Compat::STOCK, |_, _, _| {});
    assert!(stock.iter().all(|(_, ep, d)| *ep != 0x81 || d[2] & 0x02 == 0), "stock never sends F");
    let mut max_travel = 0;
    let (_, ours, _) = replay_with("held_at_boot", Compat::FIXED, |s, e, _| {
        if s > 4230 {
            max_travel = max_travel.max(e.travel(49));
        }
    });
    // F = usage 9 → byte 2 bit 1: pressed and released again after the recovery
    let f: Vec<_> = ours.iter().filter(|(_, ep, _)| *ep == 0x81).map(|(s, _, d)| (*s, d[2] & 0x02 != 0)).collect();
    assert!(f.iter().any(|&(s, on)| on && s > 4230), "{f:?}");
    assert!(f.last().is_some_and(|&(_, on)| !on));
    assert!(max_travel > 3000, "full travel after recovery, got {max_travel}");
}

#[test]
fn autocal_tracks_the_measured_bottom() {
    let fx = load("autocal");
    let bot = fx.g.bot_value(49) as i32;
    let mut stock_ref = 0;
    replay_with("autocal", Compat::STOCK, |_, e, _| stock_ref = e.autocal_ref(49));
    let mut fixed_ref = 0;
    let mut base = 0;
    replay_with("autocal", Compat::FIXED, |_, e, _| {
        fixed_ref = e.autocal_ref(49);
        base = e.rest_baseline(49);
    });
    // stock always lands on base − swing + 380 (≈ 16379 here), whatever it measured;
    // fixed converges on the measured bottom (≈ 16000 for a 16040 hold).
    assert!((stock_ref - (base - 14000 + 380)).abs() < 100, "stock {stock_ref}");
    assert!(fixed_ref < bot && (fixed_ref - 16001).abs() < 150, "fixed {fixed_ref} (stored bottom {bot})");
}

fn rig(c: Compat) -> (Engine, KeyProcessor, Profile) {
    let fx = load("press_type2");
    let mut e = Engine::new();
    let mut k = KeyProcessor::new();
    e.set_compat(c);
    k.set_compat(c);
    e.configure(&fx.g, &fx.p);
    k.invalidate();
    e.init_samples(&[30000; SLOT_COUNT]);
    e.skip_warmup();
    (e, k, fx.p)
}

fn slots(keys: &[(usize, u16)]) -> [u16; SLOT_COUNT] {
    let mut s = [30000u16; SLOT_COUNT];
    for &(k, v) in keys {
        s[OBJ_TO_SLOT[k] as usize] = v;
    }
    s
}

#[test]
fn per_slot_filter_is_not_disturbed_by_neighbours() {
    // F ramps slowly while the other keys are quiet or noisy (F's crosstalk sources stay
    // constant). With one shared integrator (stock) the noise changes F's filtered trajectory.
    let run = |c: Compat, noisy: bool| {
        let (mut e, mut k, p) = rig(c);
        let mut x = 1u32;
        let mut out = Vec::new();
        let f = OBJ_TO_SLOT[49] as usize;
        for n in 0..3000u32 {
            let mut s = [30000u16; SLOT_COUNT];
            if noisy {
                for v in s.iter_mut() {
                    x = x.wrapping_mul(1103515245).wrapping_add(12345);
                    *v = 30000 + ((x >> 16) % 61) as u16 - 30;
                }
            }
            for q in [(f + 86) % 88, (f + 6) % 88, (f + 80) % 88] {
                s[q] = 30000;
            }
            s[f] = if n < 1000 { 30000 } else { 30000 - ((n - 1000) / 4) as u16 };
            e.scan(&s);
            k.update(&e, &p);
            if n >= 1000 {
                out.push(e.raw(49));
            }
        }
        out
    };
    assert_ne!(run(Compat::STOCK, false), run(Compat::STOCK, true));
    assert_eq!(run(Compat::FIXED, false), run(Compat::FIXED, true));
}

/// Scans from the first moving sample to the press report.
fn latency(c: Compat, key: usize) -> usize {
    let (mut e, mut k, p) = rig(c);
    for _ in 0..600 {
        e.scan(&slots(&[]));
        k.update(&e, &p);
    }
    assert_eq!(e.key_pass(), KeyPass::PerStep);
    for n in 0..100 {
        e.scan(&slots(&[(key, 18000)]));
        k.update(&e, &p);
        if k.sent().iter().any(|s| s.ep == 0x81) {
            return n;
        }
    }
    panic!("no press");
}

#[test]
fn keys_are_evaluated_in_their_own_step() {
    // Esc (slot 61, step 7) is filed at step slot2key[0] = 7 by stock too; F (slot 37, step 4)
    // is filed at step 49 >> 3 = 6 → same scan; Space (slot 0) at step 76 >> 3 = 9 → same scan.
    // Q (key 31, slot 70, step 8) is filed at step 31 >> 3 = 3 → stock sees it a scan late.
    assert_eq!(latency(Compat::FIXED, 31) + 1, latency(Compat::STOCK, 31));
    for key in [0, 49, 76] {
        assert_eq!(latency(Compat::FIXED, key), latency(Compat::STOCK, key), "key {key}");
    }
}

fn run_events(p: &mut Profile, presses: &[(usize, u16)], scans: usize) -> (Vec<Event>, KeyProcessor) {
    let fx = load("press_type2");
    let mut e = Engine::new();
    let mut k = KeyProcessor::new();
    e.configure(&fx.g, p);
    k.invalidate();
    e.init_samples(&[30000; SLOT_COUNT]);
    e.skip_warmup();
    let mut ev = Vec::new();
    for n in 0..600 + scans {
        let s = if n < 600 { slots(&[]) } else { slots(presses) };
        e.scan(&s);
        ev.extend(k.update(&e, p).iter().copied());
    }
    (ev, k)
}

#[test]
fn function_keys_raise_events() {
    let fx = load("press_type2");
    for (code, want) in [
        (301, Event::Bootloader),
        (302, Event::Reboot),
        (305, Event::ResetProfile),
        (311, Event::Lighting(LightCmd::BrightnessUp)),
        (312, Event::Lighting(LightCmd::BrightnessDown)),
        (313, Event::Lighting(LightCmd::NextEffect)),
        (314, Event::Lighting(LightCmd::PrevEffect)),
        (315, Event::Lighting(LightCmd::Toggle)),
        (316, Event::WinLock(true)),
        (317, Event::Lighting(LightCmd::SpeedUp)),
        (318, Event::Lighting(LightCmd::SpeedDown)),
        (309, Event::SwitchProfile(2)),
    ] {
        let mut p = fx.p.clone();
        p.set_key_map(49, code);
        let (ev, _) = run_events(&mut p, &[(49, 18000)], 200);
        assert!(ev.contains(&want), "{code}: {ev:?}");
        // edge codes act once per press; level codes on every report rebuild while held
        assert_eq!(ev.iter().filter(|&&e| e == want).count(), 1, "{code}: {ev:?}");
    }
}

#[test]
fn fn_esc_held_five_seconds_is_a_hard_reset() {
    let fx = load("press_type2");
    let mut p = fx.p.clone();
    let (ev, _) = run_events(&mut p, &[(78, 18000), (0, 18000)], 39_990);
    assert!(!ev.contains(&Event::FactoryReset));
    let (ev, _) = run_events(&mut p, &[(78, 18000), (0, 18000)], 40_100);
    assert_eq!(ev.iter().filter(|&&e| e == Event::FactoryReset).count(), 1);
}
