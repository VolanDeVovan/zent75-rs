//! UART0 log console (PA00 TXD / PA01 RXD, 115 200 8N1, output only) — same pins, clock and
//! divisor as the stock firmware (docs/re-peripherals.md §1).
//!
//! After boot, logging never blocks the caller: `log!` formats into a RAM ring buffer and the low-priority
//! [`drain`] (called from a 1 ms task) feeds the 16-byte TX FIFO. Lines that do not fit are
//! dropped and counted. Only the panic path writes synchronously ([`write_blocking`]). Safe to
//! call from any context (the ring is guarded by a short critical section).

use core::cell::RefCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicU32, Ordering::Relaxed};

use critical_section::Mutex;
use hpm_hal::pac;

/// Ring size. At 115 200 baud the UART drains ~11.5 bytes/ms.
const RING: usize = 2048;
/// TX FIFO depth of the HPM UART.
const FIFO: usize = 16;

struct Ring {
    buf: [u8; RING],
    head: usize,
    len: usize,
    dropped: u32,
}

/// While true (boot, until the executor runs) every `log!` is written out synchronously, so a
/// hang during init still leaves its last line on the console.
static SYNC: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

/// Switch from synchronous boot logging to the buffered ring drained by [`drain`].
pub fn go_async() {
    SYNC.store(false, core::sync::atomic::Ordering::Relaxed);
}

/// Everything ever logged, last [`HIST`] bytes kept (never drained): read over USB with the
/// debug vendor command (`usb.rs`, `tools/console.py`), so no UART is needed.
const HIST: usize = 4096;
struct History {
    buf: [u8; HIST],
    /// Total bytes ever written; byte `n` lives at `buf[n % HIST]` while `n + HIST > total`.
    total: u32,
}
static HISTORY: Mutex<RefCell<History>> = Mutex::new(RefCell::new(History { buf: [0; HIST], total: 0 }));

/// Copy history bytes from position `from` into `out`; returns (total, first position copied,
/// bytes copied). Positions older than the kept window are skipped.
pub fn history(from: u32, out: &mut [u8]) -> (u32, u32, usize) {
    critical_section::with(|cs| {
        let h = HISTORY.borrow_ref(cs);
        let start = from.max(h.total.saturating_sub(HIST as u32));
        let n = (h.total.saturating_sub(start) as usize).min(out.len());
        for (i, b) in out[..n].iter_mut().enumerate() {
            *b = h.buf[(start as usize + i) % HIST];
        }
        (h.total, start, n)
    })
}

fn remember(bytes: &[u8]) {
    critical_section::with(|cs| {
        let mut h = HISTORY.borrow_ref_mut(cs);
        for &b in bytes {
            let i = h.total as usize % HIST;
            h.buf[i] = b;
            h.total = h.total.wrapping_add(1);
        }
    });
}

static LOG: Mutex<RefCell<Ring>> = Mutex::new(RefCell::new(Ring { buf: [0; RING], head: 0, len: 0, dropped: 0 }));

/// Configure UART0 exactly as stock: 24 MHz XTAL, OSCR 8, DLL 26 → 115 385 baud, FIFO on.
pub fn init() {
    use pac::sysctl::vals::ClockMux;
    let sys = pac::SYSCTL;
    hpm_hal::sysctl::clock_add_to_group(pac::resources::URT0, 0);
    sys.clock(pac::clocks::URT0).modify(|w| {
        w.set_mux(ClockMux::CLK_24M);
        w.set_div(0);
    });
    while sys.clock(pac::clocks::URT0).read().loc_busy() {}
    // PA00 = UART0_TXD, PA01 = UART0_RXD (alt 2).
    pac::IOC.pad(pac::pins::PA00).func_ctl().write(|w| w.0 = 2);
    pac::IOC.pad(pac::pins::PA01).func_ctl().write(|w| w.0 = 2);
    let u = pac::UART0;
    u.ier().write(|w| w.0 = 0);
    u.oscr().write(|w| w.0 = 8);
    u.lcr().write(|w| w.0 = 0x80); // DLAB
    u.dll().write(|w| w.0 = 26);
    u.dlm().write(|w| w.0 = 0);
    u.lcr().write(|w| w.0 = 0x03); // 8N1
    u.fcrr().write(|w| w.0 = 0x008F_0001); // stock value: FIFO enable + reset, thresholds
}

#[inline]
fn tx_empty() -> bool {
    pac::UART0.lsr().read().0 & (1 << 5) != 0 // THRE: TX FIFO empty
}

/// Append formatted text to the ring (drops the whole message if it does not fit).
pub fn write_fmt(args: fmt::Arguments) {
    struct Sink<'a>(&'a mut Ring, usize, bool);
    impl Write for Sink<'_> {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for &b in s.as_bytes() {
                if self.0.len + self.1 >= RING {
                    self.2 = true;
                    return Err(fmt::Error);
                }
                let i = (self.0.head + self.0.len + self.1) % RING;
                self.0.buf[i] = b;
                self.1 += 1;
            }
            Ok(())
        }
    }
    critical_section::with(|cs| {
        let mut r = LOG.borrow_ref_mut(cs);
        let mut s = Sink(&mut r, 0, false);
        let _ = s.write_fmt(args);
        let (n, overflow) = (s.1, s.2);
        if overflow {
            r.dropped = r.dropped.wrapping_add(1);
        } else {
            r.len += n;
        }
    });
    // The history gets the text even when the UART ring was full.
    struct Hist;
    impl Write for Hist {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            remember(s.as_bytes());
            Ok(())
        }
    }
    let _ = Hist.write_fmt(args);
    if SYNC.load(core::sync::atomic::Ordering::Relaxed) {
        flush_blocking();
    }
}

/// Move up to one FIFO's worth of bytes to the UART. Call often (≥ every 1 ms).
pub fn drain() {
    if !tx_empty() {
        return;
    }
    critical_section::with(|cs| {
        let mut r = LOG.borrow_ref_mut(cs);
        let n = r.len.min(FIFO);
        for _ in 0..n {
            let b = r.buf[r.head];
            pac::UART0.thr().write(|w| w.0 = b as u32);
            r.head = (r.head + 1) % RING;
            r.len -= 1;
        }
    });
}

/// Number of messages dropped because the ring was full.
pub fn dropped() -> u32 {
    critical_section::with(|cs| LOG.borrow_ref(cs).dropped)
}

/// Synchronous output for the panic path: flush the ring, then `s`, spinning on the FIFO.
/// Bounded: gives up on a byte after ~2 ms if the UART never becomes ready.
pub fn write_blocking(s: &str) {
    fn put(b: u8) {
        let mut spins = 0u32;
        while !tx_empty() && spins < 2_000_000 {
            spins += 1;
        }
        pac::UART0.thr().write(|w| w.0 = b as u32);
    }
    // Steal the ring contents (we may be inside an interrupt; keep the CS short per byte).
    loop {
        let b = critical_section::with(|cs| {
            let mut r = LOG.borrow_ref_mut(cs);
            if r.len == 0 {
                return None;
            }
            let b = r.buf[r.head];
            r.head = (r.head + 1) % RING;
            r.len -= 1;
            Some(b)
        });
        match b {
            Some(b) => put(b),
            None => break,
        }
    }
    for &b in s.as_bytes() {
        put(b);
    }
}

/// Wait until the UART shifted out everything (bounded).
pub fn flush_blocking() {
    write_blocking("");
    let mut spins = 0u32;
    while pac::UART0.lsr().read().0 & (1 << 6) == 0 && spins < 5_000_000 {
        spins += 1; // TEMT
    }
}

// ------------------------------------------------------------------ crash record

/// Panic text kept across the jump to the loader and the next boot, in AHB SRAM, which the
/// startup code neither loads nor zeroes. Plain atomic words (any bit pattern is a valid value),
/// checked by magic + length + checksum, so leftover garbage is never reported.
/// Layout: [0] magic, [1] length in bytes, [2] checksum, [3..] text (4 bytes per word, LE).
const CRASH_WORDS: usize = 3 + 120;
const CRASH_MAGIC: u32 = 0x5A43_5253;
#[unsafe(link_section = ".ahb_sram")]
static CRASH: [AtomicU32; CRASH_WORDS] = [const { AtomicU32::new(0) }; CRASH_WORDS];
const CRASH_TEXT: usize = 4 * (CRASH_WORDS - 3);

fn checksum(b: &[u8]) -> u32 {
    b.iter().fold(0x811C_9DC5u32, |h, &x| (h ^ x as u32).wrapping_mul(0x0100_0193))
}

/// Record a panic / stall message (keeps the first 480 bytes).
pub fn record_crash(args: fmt::Arguments) {
    struct W {
        buf: [u8; CRASH_TEXT],
        len: usize,
    }
    impl Write for W {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            for &b in s.as_bytes() {
                if self.len < self.buf.len() {
                    self.buf[self.len] = b;
                    self.len += 1;
                }
            }
            Ok(())
        }
    }
    let mut w = W { buf: [0; CRASH_TEXT], len: 0 };
    let _ = w.write_fmt(args);
    for (i, c) in w.buf.chunks(4).enumerate() {
        CRASH[3 + i].store(u32::from_le_bytes([c[0], c[1], c[2], c[3]]), Relaxed);
    }
    CRASH[1].store(w.len as u32, Relaxed);
    CRASH[2].store(checksum(&w.buf[..w.len]), Relaxed);
    CRASH[0].store(CRASH_MAGIC, Relaxed);
}

/// At boot: log a message recorded by the previous run (if any) and clear the record.
pub fn report_previous_crash() {
    let magic = CRASH[0].swap(0, Relaxed);
    let len = CRASH[1].load(Relaxed) as usize;
    if magic != CRASH_MAGIC || len > CRASH_TEXT {
        return;
    }
    let mut text = [0u8; CRASH_TEXT];
    for (i, c) in text.chunks_mut(4).enumerate() {
        c.copy_from_slice(&CRASH[3 + i].load(Relaxed).to_le_bytes());
    }
    if checksum(&text[..len]) == CRASH[2].load(Relaxed) {
        crate::log!("previous run: {}", core::str::from_utf8(&text[..len]).unwrap_or("<non-utf8>"));
    }
}

/// `log!("fmt", args…)` — one line on the console (CRLF appended).
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::console::write_fmt(format_args!("{}\r\n", format_args!($($arg)*)))
    };
}
