//! Key names, identical to `tools/rammap.py` `KEY_NAMES` (checked by a test).

use zent_core::layout::KEY_COUNT;

pub const KEY_NAMES: [&str; KEY_COUNT] = [
    "Esc", "F1", "F2", "F3", "F4", "F5", "F6", "F7", "F8", "F9", "F10", "F11", "F12", "Print", "Home", //
    "`", "1", "2", "3", "4", "5", "6", "7", "8", "9", "0", "-", "=", "Bksp", "Del", //
    "Tab", "Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P", "[", "]", "\\", "PgUp", //
    "Caps", "A", "S", "D", "F", "G", "H", "J", "K", "L", ";", "'", "Enter", "PgDn", //
    "LShift", "Z", "X", "C", "V", "B", "N", "M", ",", ".", "/", "RShift", "Up", "End", //
    "LCtrl", "LWin", "LAlt", "Space", "RAlt", "FN", "RCtrl", "Left", "Down", "Right",
];

/// Key by index (`46`) or by name (case-insensitive: `a`, `lshift`, `space`, `fn`).
pub fn parse_key(s: &str) -> Option<usize> {
    if let Ok(i) = s.parse::<usize>() {
        // A bare digit is ambiguous ("1" the key vs index 1); a leading '#' forces the name.
        return (i < KEY_COUNT).then_some(i);
    }
    let s = s.strip_prefix('#').unwrap_or(s);
    KEY_NAMES.iter().position(|n| n.eq_ignore_ascii_case(s)).or_else(|| {
        let alias = match s.to_ascii_lowercase().as_str() {
            "escape" => "Esc",
            "backspace" => "Bksp",
            "delete" => "Del",
            "pageup" => "PgUp",
            "pagedown" => "PgDn",
            "capslock" => "Caps",
            "return" => "Enter",
            "shift" => "LShift",
            "ctrl" => "LCtrl",
            "alt" => "LAlt",
            "win" | "super" => "LWin",
            "backtick" | "grave" => "`",
            "minus" => "-",
            "equal" => "=",
            "printscreen" => "Print",
            _ => return None,
        };
        KEY_NAMES.iter().position(|n| *n == alias)
    })
}

pub fn name(key: usize) -> &'static str {
    KEY_NAMES.get(key).copied().unwrap_or("?")
}
