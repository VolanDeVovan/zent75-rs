//! Debug facilities of this firmware — nothing here exists in the stock firmware or in its
//! configurator protocol (that lives in `zent_core::device::vendor`).
//!
//! * [`crash`]: panic / stall record that survives the reset, logged at the next boot.
//! * [`prof`] (cargo feature `profiler`): statistical CPU profiler.
//! * [`vendor_command`]: the debug commands on the vendor interface, so the console and the
//!   profiler work over USB with the case closed (`tools/console.py`, `tools/profile.py`).
//! * `log_key_change` (cargo feature `key-log`): key-object dump on every HID change.
//!
//! # Debug commands
//!
//! ```text
//! request  BE <cmd> 0D 00 'Z' 'D' 'B' 'G' <arg:LE32> ED
//! reply    BE <cmd> <len:LE16> <a:LE32> <b:LE32> <payload…> ED
//! ```
//!
//! | cmd  | request `arg`       | reply `a`, `b`, payload |
//! |------|---------------------|-------------------------|
//! | `D0` | console position    | bytes logged since boot, position of the first payload byte, log text |
//! | `D1` | first sample index  | samples taken, first sample index, sampled PCs (LE32 each) |
//! | `D2` | —                   | starts a new capture, then replies like `D1` |
//!
//! The command bytes are unused by stock 0.0.18, which drops unknown commands silently; the
//! fixed length and the `ZDBG` signature keep a debug frame from ever being a valid stock
//! frame of another meaning, and anything that is not exactly a debug request goes to the
//! core's dispatcher untouched. The tools only send these after reading a firmware version
//! ≥ 1.0.0 (`zentlib.Zent.debug_request`).

pub mod crash;
#[cfg(feature = "profiler")]
pub mod prof;

use zent_core::protocol::{EOF, REPORT_SIZE, SOF};

use crate::console;

const CMD_CONSOLE: u8 = 0xD0;
#[cfg(feature = "profiler")]
const CMD_PROF_READ: u8 = 0xD1;
#[cfg(feature = "profiler")]
const CMD_PROF_START: u8 = 0xD2;

const SIGNATURE: [u8; 4] = *b"ZDBG";
const REQUEST_LEN: usize = 13;
/// Reply header: SOF, cmd, len, a, b.
const HEADER_LEN: usize = 12;
/// Reply payload capacity (the frame ends with EOF).
const PAYLOAD_MAX: usize = REPORT_SIZE - HEADER_LEN - 1;

/// The `arg` of a debug request for `cmd`, or `None` if `report` is anything else.
fn parse(report: &[u8]) -> Option<(u8, u32)> {
    let r: &[u8; REQUEST_LEN] = report.get(..REQUEST_LEN)?.try_into().ok()?;
    let framed = r[0] == SOF && r[2..4] == (REQUEST_LEN as u16).to_le_bytes() && r[12] == EOF;
    (framed && r[4..8] == SIGNATURE).then(|| (r[1], u32::from_le_bytes([r[8], r[9], r[10], r[11]])))
}

/// Handle a vendor output report if it is a debug command: `true` means it was one and `reply`
/// holds the frame to send; `false` leaves it to the stock-protocol dispatcher.
pub fn vendor_command(report: &[u8], reply: &mut [u8; REPORT_SIZE]) -> bool {
    let Some((cmd, arg)) = parse(report) else {
        return false;
    };
    let payload = &mut reply[HEADER_LEN..HEADER_LEN + PAYLOAD_MAX];
    let (a, b, len) = match cmd {
        CMD_CONSOLE => console::history(arg, payload),
        #[cfg(feature = "profiler")]
        CMD_PROF_READ | CMD_PROF_START => {
            if cmd == CMD_PROF_START {
                prof::start();
            }
            let (taken, copied) = prof::samples(arg, payload);
            (taken, arg, 4 * copied)
        }
        _ => return false,
    };
    reply[0] = SOF;
    reply[1] = cmd;
    reply[2..4].copy_from_slice(&((HEADER_LEN + len + 1) as u16).to_le_bytes());
    reply[4..8].copy_from_slice(&a.to_le_bytes());
    reply[8..12].copy_from_slice(&b.to_le_bytes());
    reply[HEADER_LEN + len] = EOF;
    true
}

/// Log the HID report and every active key object. Called from the scan-processing interrupt
/// whenever the HID state changes: costs scan time, for debugging key behaviour only.
#[cfg(feature = "key-log")]
pub fn log_key_change(kb: &zent_core::device::Keyboard) {
    crate::log!("hid {:02x?}", &kb.hid().keyboard[..]);
    let (keys, engine) = (kb.keys(), kb.engine());
    for k in 0..zent_core::layout::SLOT_COUNT {
        let o = keys.object(k);
        if o.pressed != 0 || o.th != 0 || o.edge_p || o.edge_r {
            crate::log!(
                " key {} code {} pressed {} th {} mode {} travel {} raw {} p {} r {} top {} bot {} low {} peak {} press_th {} return_th {}",
                k, o.code[0], o.pressed, o.th, o.key_mode, engine.travel(k), engine.raw(k),
                o.p, o.r, o.top, o.bot, o.low, o.peak, o.press_th, o.return_th
            );
        }
    }
}
