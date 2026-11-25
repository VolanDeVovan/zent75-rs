use zent_core::config::{b2, BANK2_BASE};
use zent_core::device::Keyboard;
use zent_core::layout::{KEY_TO_SLOT, SLOT_COUNT};
use zent_core::protocol::{cmd, host, REPORT_SIZE};
use zent_core::storage::mem::MemStorage;

fn boot() -> (Keyboard, MemStorage) {
    let mut st = MemStorage::default();
    let mut kb = Keyboard::new([0x11; 16]);
    kb.boot(&mut st);
    (kb, st)
}

#[test]
fn write_read_save_switch() {
    let (mut kb, mut st) = boot();
    let mut out = [0u8; REPORT_SIZE];
    let addr = BANK2_BASE + b2::KEY_MAP as u32 + 2 * 46; // A
    assert_eq!(kb.vendor_in(&host::write(addr, &[5, 0]), &mut st, &mut out), Some(5));
    assert_eq!(&out[..5], &[0xBE, 3, 5, 0, 0xED]);
    let n = kb.vendor_in(&host::read(cmd::READ_RAM, addr, 2), &mut st, &mut out).unwrap();
    assert_eq!(n, 15);
    assert_eq!(host::read_reply(&out, cmd::READ_RAM, addr, 2), Some(&[5u8, 0][..]));
    // version is 1.0.18
    kb.vendor_in(&host::read(cmd::READ_RAM, 0x8007_0210, 4), &mut st, &mut out).unwrap();
    assert_eq!(host::read_reply(&out, cmd::READ_RAM, 0x8007_0210, 4), Some(&[18u8, 0, 1, 0][..]));
    // save, switch to profile 1 and back: edit survives
    kb.vendor_in(&host::simple(cmd::SAVE), &mut st, &mut out);
    kb.vendor_in(&host::with_arg(cmd::SWITCH_PROFILE, 1), &mut st, &mut out);
    kb.service(&mut st);
    assert_eq!(kb.active_profile(), 1);
    assert_eq!(kb.vendor_poll(&mut out), Some(6));
    assert_eq!(&out[..6], &[0xBE, 0xC0, 6, 0, 1, 0xED]);
    kb.vendor_in(&host::with_arg(cmd::SWITCH_PROFILE, 0), &mut st, &mut out);
    kb.service(&mut st);
    assert_eq!(kb.profile.key_map(46), 5);
    // out-of-range read is answered with zeros (stock leaks stack)
    kb.vendor_in(&host::read(cmd::READ_RAM, 0x8004_0000, 4), &mut st, &mut out).unwrap();
    assert_eq!(host::read_reply(&out, cmd::READ_RAM, 0x8004_0000, 4), Some(&[0u8; 4][..]));
    // serial
    assert_eq!(kb.vendor_in(&host::simple(cmd::SERIAL), &mut st, &mut out), Some(21));
}

#[test]
fn press_a_key() {
    let (mut kb, _st) = boot();
    let mut s = [30000u16; SLOT_COUNT];
    for _ in 0..13000 {
        kb.scan(&s);
    }
    assert_eq!(kb.hid().keyboard.pressed().count(), 0);
    s[KEY_TO_SLOT[46] as usize] = 20000;
    for _ in 0..50 {
        kb.scan(&s);
    }
    assert!(kb.hid().keyboard.is_pressed(4), "A pressed");
}
