//! Dispatcher semantics vs. stock 0.0.18 (docs/re-config-rgb.md §3–4) and our deliberate
//! safety differences.

use zent_core::config::{b1, b2, Bank, BANK1_BASE, BANK1_SIZE, BANK2_BASE, BANK2_SIZE};
use zent_core::defaults;
use zent_core::device::{Keyboard, SystemAction, HEARTBEAT_SCANS, VERSION_WORD, WIPE_BLINK_SCANS};
use zent_core::layout::SLOT_COUNT;
use zent_core::protocol::{cmd, host, REPORT_SIZE};
use zent_core::storage::mem::MemStorage;
use zent_core::storage::{Slot, Storage};

fn boot() -> (Keyboard, MemStorage) {
    let mut st = MemStorage::default();
    let mut kb = Keyboard::new([0x11; 16]);
    kb.boot(&mut st);
    (kb, st)
}

fn stored(st: &mut MemStorage, s: Slot, n: usize) -> Vec<u8> {
    let mut b = vec![0u8; n];
    st.load(s, &mut b).unwrap();
    b
}

#[test]
fn first_boot_writes_factory_images_and_valid_banks_stay_untouched() {
    let (kb, mut st) = boot();
    assert_eq!(kb.global.soft_version(), VERSION_WORD);
    let mut p0 = zent_core::config::Profile::zeroed();
    defaults::profile(&mut p0, 0);
    assert!(stored(&mut st, Slot::Profile(0), BANK2_SIZE) == p0.0);
    // second boot: nothing to repair, nothing written
    let n = st.stores;
    let mut kb2 = Keyboard::new([0; 16]);
    kb2.boot(&mut st);
    assert_eq!(st.stores, n);
    // a stock-written bank 1 (soft_version 0x12) loads without a rewrite either
    let mut g = zent_core::config::Global::zeroed();
    defaults::global(&mut g);
    st.store(Slot::Global, &g.0).unwrap();
    let n = st.stores;
    Keyboard::new([0; 16]).boot(&mut st);
    assert_eq!(st.stores, n);
}

#[test]
fn garbage_profile_is_repaired_on_load_and_written_back() {
    let mut st = MemStorage::default();
    let mut bad = vec![0xFFu8; BANK2_SIZE];
    bad[b2::PRESS_TH..b2::PRESS_TH + 2].copy_from_slice(&9000u16.to_le_bytes());
    st.store(Slot::Profile(0), &bad).unwrap();
    let mut kb = Keyboard::new([0; 16]);
    kb.boot(&mut st);
    assert_eq!(kb.profile.press_th(0), 50);
    assert_eq!(kb.profile.key_map(46), 4);
    assert_eq!(stored(&mut st, Slot::Profile(0), BANK2_SIZE), kb.profile.0.to_vec());
}

#[test]
fn writes_are_clipped_to_the_shadow() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    // straddles the end of the profile shadow: the part inside lands, the rest is dropped
    let addr = BANK2_BASE + BANK2_SIZE as u32 - 2;
    assert_eq!(kb.vendor_in(&host::write(addr, &[1, 2, 3, 4]), &mut st, &mut out), Some(5));
    assert_eq!(&kb.profile.0[BANK2_SIZE - 2..], &[1, 2]);
    // entirely past it: acked (stock range), nothing happens
    assert_eq!(kb.vendor_in(&host::write(BANK2_BASE + 0x4000, &[9; 16]), &mut st, &mut out), Some(5));
    // out of every range: dropped
    assert_eq!(kb.vendor_in(&host::write(0x8006_0000, &[9; 4]), &mut st, &mut out), None);
    assert_eq!(kb.vendor_in(&host::write(0x8007_7FFE, &[9; 4]), &mut st, &mut out), None);
    // reads past the shadow / outside the ranges are zeros, BE 01 is a safe shadow read
    kb.vendor_in(&host::read(cmd::READ_RAM, BANK2_BASE + BANK2_SIZE as u32 - 2, 4), &mut st, &mut out).unwrap();
    assert_eq!(host::read_reply(&out, cmd::READ_RAM, BANK2_BASE + BANK2_SIZE as u32 - 2, 4), Some(&[1u8, 2, 0, 0][..]));
    let a = BANK2_BASE + b2::KEY_MAP as u32;
    kb.vendor_in(&host::read(cmd::READ_FLASH, a, 2), &mut st, &mut out).unwrap();
    assert_eq!(host::read_reply(&out, cmd::READ_FLASH, a, 2), Some(&[41u8, 0][..]));
    kb.vendor_in(&host::read(cmd::READ_RAM, BANK1_BASE + BANK1_SIZE as u32, 2), &mut st, &mut out).unwrap();
    assert_eq!(host::read_reply(&out, cmd::READ_RAM, BANK1_BASE + BANK1_SIZE as u32, 2), Some(&[0u8, 0][..]));
    assert_eq!(zent_core::config::locate_write(0x8007_8000, 4), Some((Bank::Preview, 0)));
}

#[test]
fn bank1_write_saves_or_is_discarded() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    // plausible calibration: saved immediately, ack after
    let top: Vec<u8> = (0..83).flat_map(|_| 30000u16.to_le_bytes()).collect();
    assert_eq!(kb.vendor_in(&host::write(BANK1_BASE, &top), &mut st, &mut out), Some(5));
    assert_eq!(kb.global.top_value(5), 30000);
    assert_eq!(&stored(&mut st, Slot::Global, BANK1_SIZE)[..2], &30000u16.to_le_bytes());
    // one key with |bot - top| < 3000: the whole write is discarded (still acked)
    let n = st.stores;
    let a = BANK1_BASE + b1::BOT_VALUE as u32 + 2 * 7;
    assert_eq!(kb.vendor_in(&host::write(a, &28000u16.to_le_bytes()), &mut st, &mut out), Some(5));
    assert_eq!(kb.global.bot_value(7), 17000);
    assert_eq!(st.stores, n);
    // firmware-owned words cannot be overwritten
    let a = BANK1_BASE + b1::SOFT_VERSION as u32;
    kb.vendor_in(&host::write(a, &[0, 0, 0, 0, 0, 0, 0, 0, 3, 0, 0, 0]), &mut st, &mut out);
    assert_eq!(kb.global.soft_version(), VERSION_WORD);
    assert_eq!(kb.active_profile(), 0);
}

#[test]
fn save_reset_and_profile_switch() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    let km = BANK2_BASE + b2::KEY_MAP as u32 + 2 * 46;
    kb.vendor_in(&host::write(km, &[5, 0]), &mut st, &mut out);
    // the fixed Fn shortcut on Esc is re-imposed by the bank-1 save of BE 02
    kb.vendor_in(&host::write(BANK2_BASE + b2::KEY_MAP_FN as u32, &[4, 0]), &mut st, &mut out);
    assert_eq!(kb.profile.key_map_fn(0), 4);
    kb.vendor_in(&host::simple(cmd::SAVE), &mut st, &mut out);
    assert_eq!(kb.profile.key_map_fn(0), 319);
    assert_eq!(stored(&mut st, Slot::Profile(0), BANK2_SIZE)[b2::KEY_MAP + 92], 5);
    // switch: notice (also for the same profile), blink, not persisted without BE 02
    kb.vendor_in(&host::with_arg(cmd::SWITCH_PROFILE, 2), &mut st, &mut out);
    kb.service(&mut st);
    assert_eq!(kb.active_profile(), 2);
    assert_eq!(kb.profile.effect(), 25);
    assert_eq!(kb.profile.key_map_fn(62), 310);
    assert_eq!(kb.vendor_poll(&mut out), Some(6));
    assert_eq!(&out[..6], &[0xBE, 0xC0, 6, 0, 2, 0xED]);
    assert_eq!(stored(&mut st, Slot::Global, BANK1_SIZE)[b1::CUR_PARAM_NUM], 0);
    kb.vendor_in(&host::with_arg(cmd::SWITCH_PROFILE, 2), &mut st, &mut out);
    kb.service(&mut st);
    assert_eq!(kb.vendor_poll(&mut out), Some(6));
    // EE resets only the active profile (2 keeps WASD normal, gets effect 23), and saves it
    kb.vendor_in(&host::write(BANK2_BASE + b2::RGB_CONFIG as u32, &[0x05]), &mut st, &mut out);
    kb.vendor_in(&host::simple(cmd::RESET_CONFIG), &mut st, &mut out);
    assert_eq!(kb.profile.effect(), 23);
    assert_eq!(kb.profile.key_mode_raw(46), 0);
    assert_eq!(stored(&mut st, Slot::Profile(2), BANK2_SIZE)[b2::RGB_CONFIG], 0x5D);
    assert_eq!(stored(&mut st, Slot::Profile(0), BANK2_SIZE)[b2::KEY_MAP + 92], 5);
    // bad lengths are ignored like stock
    let mut f = host::simple(cmd::SAVE);
    f[2] = 6;
    f[4] = 0;
    f[5] = 0xED;
    let n = st.stores;
    kb.vendor_in(&f, &mut st, &mut out);
    assert_eq!(st.stores, n);
}

#[test]
fn heartbeat_lasts_16000_scans() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    kb.vendor_in(&host::with_arg(cmd::DIAG, 3), &mut st, &mut out);
    kb.vendor_in(&host::simple(cmd::HEARTBEAT), &mut st, &mut out);
    let s = [30000u16; SLOT_COUNT];
    for _ in 0..HEARTBEAT_SCANS - 1 {
        kb.scan(&s);
    }
    assert!(kb.streaming() && kb.diag_flag);
    kb.scan(&s);
    assert!(!kb.streaming());
    assert!(!kb.diag_flag);
}

#[test]
fn preview_area_is_ram_only_without_ack() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    assert_eq!(kb.vendor_in(&host::write(0x8007_8055, &[255, 0, 0]), &mut st, &mut out), None);
    assert_eq!(kb.lighting.preview_mode(), 2);
    let mut leds = [[0u8; 3]; 85];
    kb.render_leds(0, &mut leds);
    // cc(255,0,0) * V(60) / 255
    assert_eq!(leds[0], [60, 0, 0]);
    // expires 3 s after the last preview write
    kb.render_leds(2900, &mut leds);
    assert_eq!(kb.lighting.preview_mode(), 2);
    kb.render_leds(3100, &mut leds);
    assert_eq!(kb.lighting.preview_mode(), 0);
    assert_eq!(kb.vendor_in(&host::write(0x8007_8000, &[128; 85]), &mut st, &mut out), None);
    assert_eq!(kb.lighting.preview_mode(), 1);
}

#[test]
fn full_wipe_erases_everything_then_reboots() {
    let (mut kb, mut st) = boot();
    // the key layer raises Event::FactoryReset for keycode 319 held 5 s; drive it directly
    kb.request_full_wipe();
    kb.service(&mut st);
    assert!(stored(&mut st, Slot::Global, BANK1_SIZE).iter().all(|&b| b == 0xFF));
    for p in 0..4 {
        assert!(stored(&mut st, Slot::Profile(p), BANK2_SIZE).iter().all(|&b| b == 0xFF));
    }
    let s = [30000u16; SLOT_COUNT];
    for _ in 0..WIPE_BLINK_SCANS - 1 {
        kb.scan(&s);
    }
    assert_eq!(kb.take_system_action(), None);
    kb.scan(&s);
    assert_eq!(kb.take_system_action(), Some(SystemAction::Reboot));
    // the next boot is a first boot
    let mut kb2 = Keyboard::new([0; 16]);
    kb2.boot(&mut st);
    assert_eq!(kb2.profile.effect(), 23);
}
