//! Differential tests: replay traces recorded from the stock 0.0.18 engine running in unicorn
//! (`tests/fixtures/gen_engine.py`) through our engine + key pipeline with `Compat::STOCK` and
//! require exact equality of raw / travel / pressed of the watched keys after every scan, of
//! every HID report the stock firmware sent (scan, endpoint, bytes), of the BE FD frames
//! (words 0..=498; the tail statistics come from the platform) and of the calibration frames.

mod common;

use common::replay;
use zent_core::engine::compat::Compat;

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
    press_custom,
    mixed_types,
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
