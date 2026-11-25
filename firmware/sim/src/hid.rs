//! Report descriptors and report encoding for the virtual devices.

use zent_core::hid::HidReports;

/// USB vendor id of the board (the simulator keeps it so the panel's WebHID filter matches).
pub const VID: u32 = 0x36B0;
/// Product id of the simulator: deliberately NOT the real board's 0x357E.
pub const SIM_PID: u32 = 0x5A11;
/// bcdDevice-like version reported to the kernel.
pub const SIM_VERSION: u32 = 0x0118;
pub const VENDOR_NAME: &str = "ZENT75 SIM";
pub const KEYBOARD_NAME: &str = "ZENT75 SIM Keyboard";

/// Vendor raw-HID collection like interface 2 of the board: usage page 0xFF60, usage 0x61,
/// one 1024-byte input and one 1024-byte output report, no report id.
pub const VENDOR_DESCRIPTOR: &[u8] = &[
    0x06, 0x60, 0xFF, //       Usage Page (0xFF60)
    0x09, 0x61, //             Usage (0x61)
    0xA1, 0x01, //             Collection (Application)
    0x09, 0x62, //               Usage (0x62)
    0x15, 0x00, //               Logical Minimum (0)
    0x26, 0xFF, 0x00, //         Logical Maximum (255)
    0x75, 0x08, //               Report Size (8)
    0x96, 0x00, 0x04, //         Report Count (1024)
    0x81, 0x02, //               Input (Data,Var,Abs)
    0x09, 0x63, //               Usage (0x63)
    0x15, 0x00, //               Logical Minimum (0)
    0x26, 0xFF, 0x00, //         Logical Maximum (255)
    0x75, 0x08, //               Report Size (8)
    0x96, 0x00, 0x04, //         Report Count (1024)
    0x91, 0x02, //               Output (Data,Var,Abs)
    0xC0, //                   End Collection
];

pub const RID_KEYBOARD: u8 = 1;
pub const RID_CONSUMER: u8 = 2;
pub const RID_MOUSE: u8 = 3;

/// Optional `--hid-keyboard` device: 6KRO keyboard (+ LED output), one consumer usage and mouse
/// buttons, one report id each (a single uhid device instead of the board's four interfaces).
#[rustfmt::skip]
pub const KEYBOARD_DESCRIPTOR: &[u8] = &[
    // keyboard, report 1: [1, mods, 0, k1..k6]; output [1, leds]
    0x05, 0x01, 0x09, 0x06, 0xA1, 0x01, 0x85, RID_KEYBOARD,
    0x05, 0x07, 0x19, 0xE0, 0x29, 0xE7, 0x15, 0x00, 0x25, 0x01, 0x75, 0x01, 0x95, 0x08, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x08, 0x81, 0x01,
    0x95, 0x05, 0x75, 0x01, 0x05, 0x08, 0x19, 0x01, 0x29, 0x05, 0x91, 0x02,
    0x95, 0x01, 0x75, 0x03, 0x91, 0x01,
    0x95, 0x06, 0x75, 0x08, 0x15, 0x00, 0x26, 0xFF, 0x00, 0x05, 0x07, 0x19, 0x00, 0x2A, 0xFF, 0x00, 0x81, 0x00,
    0xC0,
    // consumer, report 2: [2, usage LE16]
    0x05, 0x0C, 0x09, 0x01, 0xA1, 0x01, 0x85, RID_CONSUMER,
    0x15, 0x00, 0x26, 0xFF, 0x03, 0x19, 0x00, 0x2A, 0xFF, 0x03, 0x75, 0x10, 0x95, 0x01, 0x81, 0x00,
    0xC0,
    // mouse, report 3: [3, buttons, x, y]
    0x05, 0x01, 0x09, 0x02, 0xA1, 0x01, 0x85, RID_MOUSE,
    0x09, 0x01, 0xA1, 0x00,
    0x05, 0x09, 0x19, 0x01, 0x29, 0x05, 0x15, 0x00, 0x25, 0x01, 0x95, 0x05, 0x75, 0x01, 0x81, 0x02,
    0x95, 0x01, 0x75, 0x03, 0x81, 0x01,
    0x05, 0x01, 0x09, 0x30, 0x09, 0x31, 0x15, 0x81, 0x25, 0x7F, 0x75, 0x08, 0x95, 0x02, 0x81, 0x06,
    0xC0, 0xC0,
];

pub fn keyboard_report(h: &HidReports) -> [u8; 9] {
    let mut r = [0u8; 9];
    r[0] = RID_KEYBOARD;
    r[1..].copy_from_slice(&h.boot_report());
    r
}
pub fn consumer_report(h: &HidReports) -> [u8; 3] {
    let u = h.consumer_usage().to_le_bytes();
    [RID_CONSUMER, u[0], u[1]]
}
pub fn mouse_report(h: &HidReports) -> [u8; 4] {
    let [buttons, x, y] = h.mouse_report();
    [RID_MOUSE, buttons, x, y]
}

/// One-line description of the HID state for logs and `state`.
pub fn describe(h: &HidReports) -> String {
    let keys: Vec<String> = h.pressed().map(|u| format!("{u:02x}")).collect();
    format!(
        "mods={:02x} keys={} consumer={:04x} mouse={:02x}",
        h.modifiers(),
        if keys.is_empty() { "-".to_string() } else { keys.join(",") },
        h.consumer_usage(),
        h.mouse_buttons
    )
}
