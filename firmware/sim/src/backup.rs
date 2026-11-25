//! Seed the simulator's storage from a `tools/dump_all.py` backup (`backups/<ts>/dump.json`).
//!
//! `dump.json` holds, per profile `"0".."3"`, every RAM-map region as
//! `{"addr": <u32>, "len": <n>, "data": "<hex>"}`. Profile regions (`0x8008xxxx`) go into that
//! profile's bank-2 blob; board-global regions (`0x8007xxxx`) are taken from the profile that
//! was active when the dump was made. `soft_version` is skipped (the firmware sets it on boot)
//! and `cur_param_num` comes from the dump's `active_profile`.

use std::path::Path;

use zent_core::config::{self, BANK1_SIZE, BANK2_SIZE, Bank, Global, Profile};
use zent_core::defaults;
use zent_core::layout::PROFILE_COUNT;
use zent_core::storage::{Slot, Storage};

use crate::json::{self, Value};

#[derive(Debug, Default)]
pub struct Summary {
    pub active_profile: usize,
    pub firmware: String,
    pub regions: usize,
    pub skipped: Vec<String>,
}

fn hex(s: &str) -> Result<Vec<u8>, String> {
    if !s.len().is_multiple_of(2) {
        return Err("odd hex length".into());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string()))
        .collect()
}

fn region(v: &Value) -> Result<(u32, Vec<u8>), String> {
    let addr = v
        .get("addr")
        .and_then(Value::as_u64)
        .ok_or("region without addr")? as u32;
    let len = v
        .get("len")
        .and_then(Value::as_u64)
        .ok_or("region without len")? as usize;
    let data = hex(v
        .get("data")
        .and_then(Value::as_str)
        .ok_or("region without data")?)?;
    if data.len() != len {
        return Err(format!(
            "region {addr:#x}: {} data bytes, len {len}",
            data.len()
        ));
    }
    Ok((addr, data))
}

/// Load `dir/dump.json` (or a dump.json path) and store bank 1 and all four profiles.
pub fn load_backup(path: &Path, st: &mut impl Storage) -> Result<Summary, String> {
    let file = if path.is_dir() {
        path.join("dump.json")
    } else {
        path.to_path_buf()
    };
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {e}", file.display()))?;
    let root = json::parse(&text)?;
    let profiles = root
        .get("profiles")
        .and_then(Value::as_obj)
        .ok_or("dump.json has no profiles")?;
    let active = root
        .get("active_profile")
        .and_then(Value::as_u64)
        .unwrap_or(0) as usize;
    let active = active.min(PROFILE_COUNT - 1);
    let mut sum = Summary {
        active_profile: active,
        firmware: root
            .get("firmware")
            .and_then(Value::as_str)
            .unwrap_or("?")
            .to_string(),
        ..Default::default()
    };

    let mut global = Global::zeroed();
    defaults::global(&mut global);
    for p in 0..PROFILE_COUNT {
        let Some(regs) = profiles.get(&p.to_string()).and_then(Value::as_obj) else {
            sum.skipped.push(format!("profile {p} missing (defaults)"));
            continue;
        };
        let mut prof = Profile::zeroed();
        defaults::profile(&mut prof, p);
        for (name, v) in regs {
            let (addr, data) = region(v).map_err(|e| format!("{name}: {e}"))?;
            match config::locate_read(addr, data.len()) {
                Some((Bank::Profile, off)) if off + data.len() <= BANK2_SIZE => {
                    prof.0[off..off + data.len()].copy_from_slice(&data);
                    sum.regions += 1;
                }
                Some((Bank::Global, off)) if off + data.len() <= BANK1_SIZE => {
                    if p == active && name != "soft_version" && name != "cur_param_num" {
                        global.0[off..off + data.len()].copy_from_slice(&data);
                        sum.regions += 1;
                    }
                }
                _ => sum.skipped.push(format!(
                    "{name} @ {addr:#x}+{} outside the simulated banks",
                    data.len()
                )),
            }
        }
        st.store(Slot::Profile(p as u8), &prof.0)
            .map_err(|e| format!("store profile {p}: {e:?}"))?;
    }
    global.set_active_profile(active);
    st.store(Slot::Global, &global.0)
        .map_err(|e| format!("store global: {e:?}"))?;
    Ok(sum)
}
