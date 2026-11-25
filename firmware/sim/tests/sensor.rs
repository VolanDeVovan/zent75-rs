use zent_core::layout::{KEY_COUNT, KEY_TO_SLOT, NO_KEY, SCAN_HZ, SLOT_TO_KEY};
use zent_sim::sensor::*;

#[test]
fn rest_and_bottom_levels() {
    let m = SensorModel::new(7, 0, 0.0);
    for k in 0..KEY_COUNT {
        let rest = m.level(k, 0.0) as i32;
        let bottom = m.level(k, FULL_TRAVEL_MM) as i32;
        assert!((rest - 30000).abs() <= REST_SPREAD, "key {k} rest {rest}");
        assert!(
            (bottom - 18000).abs() <= BOTTOM_SPREAD,
            "key {k} bottom {bottom}"
        );
        let mid = m.level(k, FULL_TRAVEL_MM / 2.0) as i32;
        assert!((mid - (rest + bottom) / 2).abs() <= 1);
        // beyond the ends it clamps
        assert_eq!(m.level(k, 99.0) as i32, bottom);
    }
    // per-key variation exists
    assert!((0..KEY_COUNT).any(|k| m.level(k, 0.0) != m.level(0, 0.0)));
}

#[test]
fn samples_follow_the_slot_map_and_noise_is_bounded() {
    let mut m = SensorModel::new(1, 25, 0.0);
    m.set_now(46, FULL_TRAVEL_MM);
    for _ in 0..2000 {
        let s = m.samples();
        for (slot, &k) in SLOT_TO_KEY.iter().enumerate() {
            if k == NO_KEY {
                assert_eq!(s[slot], SPARE_SLOT);
                continue;
            }
            let k = k as usize;
            let want = m.level(k, m.travel(k)) as i32;
            assert!((s[slot] as i32 - want).abs() <= 25, "slot {slot}");
        }
        assert!(s[KEY_TO_SLOT[46] as usize] < 19000);
    }
}

#[test]
fn slew_limits_travel_speed() {
    // 200 mm/s → 0.025 mm per scan → full travel in 140 scans
    let mut m = SensorModel::new(1, 0, 200.0);
    m.set_target(3, FULL_TRAVEL_MM);
    m.step();
    assert!((m.travel(3) - 200.0 / SCAN_HZ as f32).abs() < 1e-6);
    for _ in 0..200 {
        m.step();
    }
    assert_eq!(m.travel(3), FULL_TRAVEL_MM);
    m.set_target(3, 0.0);
    for _ in 0..200 {
        m.step();
    }
    assert_eq!(m.travel(3), 0.0);
}
