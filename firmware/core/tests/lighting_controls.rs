//! Lighting behaviour the stock harness cannot render (keys, sleep, suspend, profile blink,
//! WinLock), checked against the stock semantics in docs/re-config-rgb.md §4–5.

use zent_core::config::{b2, Profile};
use zent_core::defaults;
use zent_core::keys::LightCmd;
use zent_core::layout::{KEY_TO_LED, LED_COUNT};
use zent_core::lighting::{Lighting, BLINK_TICKS, PROFILE_BLINK};

fn setup(effect: u8) -> (Lighting, Profile) {
    let mut p = Profile::zeroed();
    defaults::profile(&mut p, 0);
    p.set_effect(effect);
    let mut l = Lighting::new();
    l.configure(&mut p);
    (l, p)
}

fn frame(l: &mut Lighting, p: &Profile, t: u32) -> [[u8; 3]; LED_COUNT] {
    let mut out = [[0; 3]; LED_COUNT];
    l.render_at_tick(t, p, &mut out);
    out
}

#[test]
fn effect_keys_cycle_1_to_50_skipping_2() {
    let (mut l, mut p) = setup(1);
    l.command(LightCmd::NextEffect, &mut p);
    assert_eq!(p.effect(), 3);
    assert_eq!(p.0[b2::RGB_MODE] & 3, 1);
    l.command(LightCmd::PrevEffect, &mut p);
    assert_eq!(p.effect(), 1);
    l.command(LightCmd::PrevEffect, &mut p);
    assert_eq!(p.effect(), 50);
    l.command(LightCmd::NextEffect, &mut p);
    assert_eq!(p.effect(), 1);
    assert_eq!(l.params().0, 1);
}

#[test]
fn brightness_and_speed_keys() {
    let (mut l, mut p) = setup(1);
    assert_eq!(l.params().3, 60);
    for _ in 0..20 {
        l.command(LightCmd::BrightnessUp, &mut p);
    }
    assert_eq!(l.params().3, 180);
    assert_eq!(p.0[b2::RGB_BRIGHTNESS], 30);
    for _ in 0..20 {
        l.command(LightCmd::BrightnessDown, &mut p);
    }
    assert_eq!((l.params().3, p.0[b2::RGB_BRIGHTNESS]), (0, 0));
    // speed is runtime only: lost at the next reload
    l.command(LightCmd::SpeedUp, &mut p);
    assert_eq!(l.params().4, 0x32 + 17);
    assert_eq!(p.0[b2::RGB_SPEED], 0x32);
    l.configure(&mut p);
    assert_eq!(l.params().4, 0x32);
    // brightness above 30 in the profile is clamped and written back by the reload
    p.0[b2::RGB_BRIGHTNESS] = 31;
    l.configure(&mut p);
    assert_eq!((l.params().3, p.0[b2::RGB_BRIGHTNESS]), (180, 30));
}

#[test]
fn toggle_and_usb_suspend_blank_the_leds() {
    let (mut l, mut p) = setup(1);
    assert_ne!(frame(&mut l, &p, 0)[0], [0; 3]);
    l.command(LightCmd::Toggle, &mut p);
    assert_eq!(p.0[b2::RGB_ENABLE], 0);
    assert!(frame(&mut l, &p, 8).iter().all(|c| *c == [0; 3]));
    l.command(LightCmd::Toggle, &mut p);
    assert_ne!(frame(&mut l, &p, 16)[0], [0; 3]);
    l.set_usb_suspended(true);
    assert!(frame(&mut l, &p, 24).iter().all(|c| *c == [0; 3]));
    l.set_usb_suspended(false);
    assert_ne!(frame(&mut l, &p, 32)[0], [0; 3]);
}

#[test]
fn sleep_blanks_after_idle_and_a_key_wakes() {
    let (mut l, mut p) = setup(1);
    p.0[b2::SLEEP_SECOND_CNT..b2::SLEEP_SECOND_CNT + 4].copy_from_slice(&1u32.to_le_bytes());
    l.configure(&mut p);
    let keys = zent_core::keys::KeyProcessor::new();
    for _ in 0..2 * 8000 {
        l.on_scan(&keys);
    }
    assert_eq!(l.idle_seconds(), 2);
    assert!(frame(&mut l, &p, 0).iter().all(|c| *c == [0; 3]));
    l.note_activity();
    assert_ne!(frame(&mut l, &p, 8)[0], [0; 3]);
}

#[test]
fn profile_blink_three_flashes_in_1000_ticks() {
    let (mut l, p) = setup(1);
    l.start_profile_blink(3);
    let solid = frame(&mut l, &p, 1000)[10]; // blink starts here, on-phase
    assert_eq!(solid, PROFILE_BLINK[3]);
    assert_eq!(frame(&mut l, &p, 1250)[10], [0; 3]);
    assert_eq!(frame(&mut l, &p, 1450)[10], PROFILE_BLINK[3]);
    assert_eq!(frame(&mut l, &p, 1850)[10], PROFILE_BLINK[3]);
    assert_eq!(frame(&mut l, &p, 1000 + BLINK_TICKS as u32 + 1)[10], [0; 3]);
    // over: the effect is back
    assert_ne!(frame(&mut l, &p, 2100)[10], [0; 3]);
}

#[test]
fn winlock_indicator() {
    let (mut l, mut p) = setup(1);
    let lwin = KEY_TO_LED[74] as usize;
    l.set_winlock(true);
    assert_eq!(frame(&mut l, &p, 0)[lwin], [255, 0, 0]);
    // not while Fn is held (Fn layer: LWin carries the fixed WinLock shortcut → white)
    l.set_fn_held(true);
    assert_eq!(frame(&mut l, &p, 8)[lwin], [60, 60, 60]);
    l.set_fn_held(false);
    p.0[b2::WIN_LOCK_STATE] = 0;
    assert_ne!(frame(&mut l, &p, 16)[lwin], [255, 0, 0]);
}

#[test]
fn effect_none_renders_black_once_then_holds() {
    let (mut l, p) = setup(0);
    assert!(frame(&mut l, &p, 0).iter().all(|c| *c == [0; 3]));
    l.set_host_leds(2); // Caps: stock stops flushing with effect 0, so it does not show
    assert!(frame(&mut l, &p, 8).iter().all(|c| *c == [0; 3]));
}
