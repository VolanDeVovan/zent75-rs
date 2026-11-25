use std::path::PathBuf;

use zent_sim::command::{parse, Command};
use zent_sim::keynames::{parse_key, KEY_NAMES};
use zent_sim::sensor::FULL_TRAVEL_MM;

/// KEY_NAMES must match tools/rammap.py exactly.
#[test]
fn names_match_rammap_py() {
    let py = std::fs::read_to_string(PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tools/rammap.py")).unwrap();
    let start = py.find("KEY_NAMES = (").unwrap();
    let end = start + py[start..].find(").split()").unwrap();
    let mut joined = String::new();
    for lit in py[start..end].split('"').skip(1).step_by(2) {
        joined.push_str(&lit.replace("\\\\", "\\"));
    }
    let names: Vec<&str> = joined.split_whitespace().collect();
    assert_eq!(names, KEY_NAMES.to_vec());
}

#[test]
fn key_parsing() {
    assert_eq!(parse_key("a"), Some(46));
    assert_eq!(parse_key("LShift"), Some(59));
    assert_eq!(parse_key("space"), Some(76));
    assert_eq!(parse_key("Fn"), Some(78));
    assert_eq!(parse_key("82"), Some(82));
    assert_eq!(parse_key("83"), None);
    assert_eq!(parse_key("#1"), Some(16));
    assert_eq!(parse_key("\\"), Some(43));
    assert_eq!(parse_key("nope"), None);
}

#[test]
fn command_parsing() {
    assert_eq!(parse("press A").unwrap(), Some(Command::Press { key: 46, mm: FULL_TRAVEL_MM }));
    assert_eq!(parse("  travel s 1.2 ").unwrap(), Some(Command::Press { key: 47, mm: 1.2 }));
    assert_eq!(parse("release all").unwrap(), Some(Command::Release { key: None }));
    assert_eq!(parse("tap Esc").unwrap(), Some(Command::Tap { key: 0, ms: 30 }));
    assert_eq!(parse("hold-at-boot A S D").unwrap(), Some(Command::HoldAtBoot(vec![46, 47, 48])));
    assert_eq!(parse("# comment").unwrap(), None);
    assert_eq!(parse("").unwrap(), None);
    assert!(parse("press").is_err());
    assert!(parse("press A 9").is_err());
    assert!(parse("frobnicate").is_err());
    assert_eq!(parse("sleep 20").unwrap(), Some(Command::Sleep(20)));
}
