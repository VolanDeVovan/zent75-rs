//! Crash record: the text of a panic or of a thread-mode stall, kept across the reset (or the
//! trip through the UF2 loader) and logged by the next boot.
//!
//! The record lives in AHB SRAM, which the startup code neither loads nor zeroes. It is plain
//! atomic words (any bit pattern is a valid value) validated by magic + length + checksum, so
//! leftover garbage is never reported.

use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

/// Layout: `[0]` magic, `[1]` text length in bytes, `[2]` checksum, `[3..]` text (LE words).
const HEADER_WORDS: usize = 3;
const TEXT_LEN: usize = 480;
const MAGIC: u32 = 0x5A43_5253;

// SAFETY: `.ahb_sram` is a NOLOAD RAM section of `zent-link.x`; the words are validated on read.
#[unsafe(link_section = ".ahb_sram")]
static RECORD: [AtomicU32; HEADER_WORDS + TEXT_LEN / 4] = [const { AtomicU32::new(0) }; HEADER_WORDS + TEXT_LEN / 4];

/// FNV-1a.
fn checksum(text: &[u8]) -> u32 {
    text.iter().fold(0x811C_9DC5u32, |h, &b| (h ^ b as u32).wrapping_mul(0x0100_0193))
}

/// Record a message for the next boot (the first [`TEXT_LEN`] bytes of it).
pub fn record(args: fmt::Arguments) {
    struct Text {
        buf: [u8; TEXT_LEN],
        len: usize,
    }
    impl Write for Text {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let n = s.len().min(TEXT_LEN - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }
    let mut text = Text { buf: [0; TEXT_LEN], len: 0 };
    let _ = text.write_fmt(args);
    RECORD[0].store(0, Relaxed); // invalid while it is being rewritten
    for (word, bytes) in RECORD[HEADER_WORDS..].iter().zip(text.buf.as_chunks::<4>().0) {
        word.store(u32::from_le_bytes(*bytes), Relaxed);
    }
    RECORD[1].store(text.len as u32, Relaxed);
    RECORD[2].store(checksum(&text.buf[..text.len]), Relaxed);
    RECORD[0].store(MAGIC, Relaxed);
}

/// At boot: log the message recorded by the previous run, if any, and clear the record.
pub fn report_previous() {
    let magic = RECORD[0].swap(0, Relaxed);
    let len = RECORD[1].load(Relaxed) as usize;
    if magic != MAGIC || len > TEXT_LEN {
        return;
    }
    let mut text = [0u8; TEXT_LEN];
    for (bytes, word) in text.as_chunks_mut::<4>().0.iter_mut().zip(&RECORD[HEADER_WORDS..]) {
        *bytes = word.load(Relaxed).to_le_bytes();
    }
    let text = &text[..len];
    if checksum(text) == RECORD[2].load(Relaxed) {
        // a message cut at TEXT_LEN may end inside a UTF-8 sequence: log the valid part
        let valid = match core::str::from_utf8(text) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&text[..e.valid_up_to()]).unwrap_or(""),
        };
        crate::log!("previous run: {}", valid);
    }
}

// ------------------------------------------------------------------ stall detector

/// Scans between two checks (250 ms at 8 kHz).
const CHECK_EVERY_SCANS: u32 = 2000;
/// Checks without a heartbeat before the stall is recorded: 1.25–1.5 s after the last one,
/// ahead of the 2 s watchdog.
const STALLED_CHECKS: u32 = 5;
const _: () = assert!((STALLED_CHECKS as u64 + 1) * 250 < crate::system::WATCHDOG_TIMEOUT_MS);

/// Bumped with every watchdog feed ([`heartbeat`]).
static BEAT: AtomicU32 = AtomicU32::new(0);

/// Thread mode is alive. Called by `system::watchdog_feed`.
pub fn heartbeat() {
    BEAT.fetch_add(1, Relaxed);
}

/// Run from the scan-processing interrupt after every scan: when the watchdog has not been fed
/// for [`STALLED_CHECKS`] checks, record the `pc` thread mode is stuck at. The watchdog reset
/// follows, the boot after it goes to the UF2 loader, and the next start of this firmware logs
/// the record.
pub fn stall_check(scans: u32, pc: u32) {
    static LAST_BEAT: AtomicU32 = AtomicU32::new(u32::MAX);
    static MISSED: AtomicU32 = AtomicU32::new(0);
    if !scans.is_multiple_of(CHECK_EVERY_SCANS) {
        return;
    }
    let beat = BEAT.load(Relaxed);
    if beat != LAST_BEAT.swap(beat, Relaxed) {
        MISSED.store(0, Relaxed);
    } else if MISSED.fetch_add(1, Relaxed) + 1 == STALLED_CHECKS {
        record(format_args!("thread mode stalled at pc {:#010x}", pc));
        crate::log!("thread mode stalled at pc {:#010x}", pc);
    }
}
