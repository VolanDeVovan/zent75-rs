//! Config model vs. the stock firmware: factory images and boot validation/repair, checked
//! against images the stock 0.0.18 code produced in the emulator (fixtures/gen_rgb.py).

use std::fs;
use std::path::PathBuf;
use zent_core::config::{self, Global, Profile, BANK1_SIZE, BANK2_SIZE};
use zent_core::defaults;

fn fx(p: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/config").join(p)
}

fn diff(a: &[u8], b: &[u8]) -> Vec<String> {
    (0..a.len()).filter(|&i| a[i] != b[i]).take(12).map(|i| format!("{i:#x}: {:02x} want {:02x}", a[i], b[i])).collect()
}

#[test]
fn factory_images_match_stock() {
    let mut g = Global::zeroed();
    defaults::global(&mut g);
    let want = fs::read(fx("bank1.bin")).unwrap();
    assert_eq!(want.len(), BANK1_SIZE);
    assert!(g.0[..] == want[..], "bank1 {:?}", diff(&g.0, &want));
    for i in 0..4 {
        let mut p = Profile::zeroed();
        defaults::profile(&mut p, i);
        let want = fs::read(fx(&format!("profile{i}.bin"))).unwrap();
        assert_eq!(want.len(), BANK2_SIZE);
        assert!(p.0[..] == want[..], "profile {i} {:?}", diff(&p.0, &want));
    }
}

#[test]
fn repair_matches_stock() {
    let mut n = 0;
    for e in fs::read_dir(fx("repair")).unwrap() {
        let path = e.unwrap().path();
        let name = path.file_name().unwrap().to_str().unwrap().to_string();
        let Some(stem) = name.strip_suffix(".in") else { continue };
        let input = fs::read(&path).unwrap();
        let want = fs::read(fx(&format!("repair/{stem}.out"))).unwrap();
        if let Some(rest) = stem.strip_prefix("b1_") {
            let mut g = Global(input.try_into().unwrap());
            g.repair(config::STOCK_SOFT_VERSION);
            assert!(g.0[..] == want[..], "{rest}: {:?}", diff(&g.0, &want));
        } else {
            let idx: usize = stem[1..2].parse().unwrap();
            let mut p = Profile(input.try_into().unwrap());
            p.load_fixup(idx);
            p.clamp_brightness(); // stock's lighting reload at the end of the load
            assert!(p.0[..] == want[..], "{stem}: {:?}", diff(&p.0, &want));
        }
        n += 1;
    }
    assert!(n > 50);
}

#[test]
fn stock_written_banks_load_unchanged() {
    // a valid stock bank must not be touched by the load-time checks
    for i in 0..4 {
        let want = fs::read(fx(&format!("profile{i}.bin"))).unwrap();
        let mut p = Profile(want.clone().try_into().unwrap());
        assert!(!p.load_fixup(i));
        assert!(p.0[..] == want[..]);
    }
    let want = fs::read(fx("bank1.bin")).unwrap();
    let mut g = Global(want.clone().try_into().unwrap());
    assert!(!g.repair(config::STOCK_SOFT_VERSION));
    assert!(g.0[..] == want[..]);
}
