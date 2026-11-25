//! Engine + key pipeline wired into `Keyboard`: calibration frames, DA, Fn + Esc hard reset,
//! WinLock indicator, `BE C0` report freeze.

use zent_core::config::BANK1_SIZE;
use zent_core::device::Keyboard;
use zent_core::layout::{KEY_TO_SLOT, SLOT_COUNT};
use zent_core::protocol::{cmd, host, REPORT_SIZE};
use zent_core::storage::mem::MemStorage;
use zent_core::storage::{Slot, Storage};

fn boot() -> (Keyboard, MemStorage) {
    let mut st = MemStorage::default();
    let mut kb = Keyboard::new([0x11; 16]);
    kb.boot(&mut st);
    (kb, st)
}

fn samples(pressed: &[usize]) -> [u16; SLOT_COUNT] {
    let mut s = [30000u16; SLOT_COUNT];
    for &k in pressed {
        s[KEY_TO_SLOT[k] as usize] = 18000;
    }
    s
}

fn settle(kb: &mut Keyboard) {
    let s = samples(&[]);
    for _ in 0..13000 {
        kb.scan(&s);
    }
    assert!(kb.engine_ready());
}

#[test]
fn calibration_frames_without_heartbeat() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    kb.vendor_in(&host::with_arg(cmd::CALIBRATION, 1), &mut st, &mut out);
    let s = samples(&[]);
    let mut frames = Vec::new();
    for n in 0..8000 + 48 * 5 {
        kb.scan(&s);
        if let Some(len) = kb.vendor_poll(&mut out) {
            assert_eq!(len, 1003);
            assert_eq!(&out[..4], &[0xBE, 0xFD, 0xEB, 0x03]);
            assert_eq!(u16::from_le_bytes([out[4], out[5]]), 30000);
            frames.push(n);
        }
    }
    // after 1 s settling, one frame per 48 scans (~167/s)
    assert_eq!(frames.len(), 5, "{frames:?}");
    assert!(frames.windows(2).all(|w| w[1] - w[0] == 48));
    // no keys in calibration mode
    let s = samples(&[46]);
    for _ in 0..100 {
        kb.scan(&s);
    }
    assert_eq!(kb.hid().keyboard.pressed().count(), 0);
}

#[test]
fn fn_esc_five_seconds_wipes_but_keeps_calibration() {
    let (mut kb, mut st) = boot();
    kb.global.set_top_value(3, 30500);
    let mut out = [0u8; REPORT_SIZE];
    kb.vendor_in(&host::simple(cmd::SAVE), &mut st, &mut out);
    settle(&mut kb);
    let s = samples(&[78, 0]); // Fn + Esc
    for _ in 0..40_100 {
        kb.scan(&s);
        kb.service(&mut st);
    }
    let mut g = [0u8; BANK1_SIZE];
    st.load(Slot::Global, &mut g).unwrap();
    assert_eq!(u16::from_le_bytes([g[6], g[7]]), 30500, "calibration kept");
    assert!(g[0x210..].iter().all(|&b| b == 0xFF), "rest of bank 1 erased");
}

#[test]
fn winlock_reaches_the_lighting_and_blocks_win() {
    let (mut kb, _st) = boot();
    settle(&mut kb);
    for keys in [&[78usize][..], &[78, 74], &[78], &[]] {
        let s = samples(keys);
        for _ in 0..100 {
            kb.scan(&s);
        }
    }
    assert!(kb.lighting.winlock());
    let s = samples(&[74]);
    for _ in 0..100 {
        kb.scan(&s);
    }
    assert_eq!(kb.hid().keyboard.modifiers & 0x88, 0, "LWin blocked");
}

#[test]
fn host_profile_switch_freezes_reports() {
    let (mut kb, mut st) = boot();
    settle(&mut kb);
    let mut out = [0u8; REPORT_SIZE];
    kb.vendor_in(&host::with_arg(cmd::SWITCH_PROFILE, 1), &mut st, &mut out);
    let s = samples(&[49]);
    for _ in 0..7300 {
        kb.scan(&s);
        kb.service(&mut st);
    }
    // F went down during the cool-down: no rebuild then, and none after it ends (stock)
    assert_eq!(kb.hid().keyboard.pressed().count(), 0, "frozen");
    // the next change after the cool-down rebuilds the report
    let s = samples(&[49, 52]);
    for _ in 0..400 {
        kb.scan(&s);
    }
    assert!(kb.hid().keyboard.is_pressed(9) && kb.hid().keyboard.is_pressed(13));
}
