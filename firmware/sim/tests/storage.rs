use std::path::PathBuf;

use zent_core::config::{b2, BANK2_BASE, BANK1_SIZE, BANK2_SIZE};
use zent_core::protocol::{cmd, host};
use zent_core::storage::{Slot, Storage, StorageError};
use zent_sim::sim::{Options, Sim};
use zent_sim::store::DirStorage;

fn tmpdir(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("zent-sim-test-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

#[test]
fn dir_storage_roundtrip() {
    let d = tmpdir("rt");
    let mut st = DirStorage::open(&d).unwrap();
    let mut buf = [0u8; 8];
    assert_eq!(st.load(Slot::Global, &mut buf), Err(StorageError::Empty));
    st.store(Slot::Profile(2), &[1, 2, 3]).unwrap();
    let mut st2 = DirStorage::open(&d).unwrap();
    buf.fill(9);
    st2.load(Slot::Profile(2), &mut buf).unwrap();
    assert_eq!(buf, [1, 2, 3, 0, 0, 0, 0, 0]);
    assert!(d.join("profile2.bin").exists());
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn saves_survive_a_sim_restart() {
    let d = tmpdir("persist");
    let addr = BANK2_BASE + b2::KEY_MAP as u32 + 2 * 46; // A
    {
        let mut sim = Sim::new(Options::default(), DirStorage::open(&d).unwrap()).unwrap();
        assert_eq!(std::fs::read(d.join("global.bin")).unwrap().len(), BANK1_SIZE);
        let r = sim.vendor_report(&host::write(addr, &[5, 0])).unwrap();
        assert_eq!(&r[..5], &[0xBE, 3, 5, 0, 0xED]);
        assert!(sim.vendor_report(&host::simple(cmd::SAVE)).is_none());
        // unsaved edit on profile 0 after the save is lost by the profile switch below
        sim.vendor_report(&host::with_arg(cmd::SWITCH_PROFILE, 2));
        sim.tick(8);
    }
    let mut sim = Sim::new(Options::default(), DirStorage::open(&d).unwrap()).unwrap();
    assert_eq!(sim.kb.active_profile(), 2, "active profile persisted");
    // 1025-byte hidraw form (report id 0) is accepted too
    let mut r = vec![0u8];
    r.extend_from_slice(&host::with_arg(cmd::SWITCH_PROFILE, 0));
    sim.vendor_report(&r);
    assert_eq!(sim.kb.profile.key_map(46), 5);
    assert_eq!(std::fs::read(d.join("profile0.bin")).unwrap().len(), BANK2_SIZE);
    drop(sim);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn load_backup_seeds_all_profiles() {
    let bk = repo().join("backups/1795545170");
    if !bk.join("dump.json").exists() {
        eprintln!("no backup in the repo, skipping");
        return;
    }
    let d = tmpdir("backup");
    let mut st = DirStorage::open(&d).unwrap();
    let sum = zent_sim::backup::load_backup(&bk, &mut st).unwrap();
    assert!(sum.skipped.is_empty(), "{:?}", sum.skipped);
    let mut sim = Sim::new(Options::default(), st).unwrap();
    assert_eq!(sim.kb.active_profile(), sum.active_profile);
    for p in 0..4u8 {
        sim.vendor_report(&host::with_arg(cmd::SWITCH_PROFILE, p));
        let want = std::fs::read(bk.join(format!("p{p}_key_map.bin"))).unwrap();
        assert_eq!(&sim.kb.profile.0[b2::KEY_MAP..b2::KEY_MAP + 166], &want[..], "profile {p} key_map");
        let want = std::fs::read(bk.join(format!("p{p}_bind_key_setting_46.bin"))).unwrap();
        assert_eq!(&sim.kb.profile.bind_record(46)[..], &want[..]);
    }
    let top = std::fs::read(bk.join("p0_top_value.bin")).unwrap();
    assert_eq!(&sim.kb.global.0[..166], &top[..]);
    let name = std::fs::read(bk.join("p0_config_name1.bin")).unwrap();
    assert_eq!(&sim.kb.global.0[0x290..0x290 + 64], &name[..]);
    drop(sim);
    std::fs::remove_dir_all(&d).unwrap();
}
