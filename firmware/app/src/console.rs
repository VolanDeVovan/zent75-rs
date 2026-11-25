//! Log console: UART0 (PA00 TXD / PA01 RXD, 115 200 8N1, output only — same pins, clock and
//! divisor as the stock firmware, docs/re-peripherals.md §1) plus a history that the host reads
//! over USB (`debug`, `tools/console.py`), so the case can stay closed.
//!
//! `log!` formats the line on the caller's stack, then appends it under a short critical
//! section to the UART ring (drained by a 1 ms task into the 16-byte TX FIFO; a line that does
//! not fit is dropped and counted) and to the history (the last [`HIST`] bytes, never drained).
//! It never blocks and may be called from any context. During boot, until [`go_async`], and on
//! the panic path the UART is written synchronously instead.

use core::cell::RefCell;
use core::fmt::{self, Write};
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

use critical_section::Mutex;
use hpm_hal::pac;

/// UART ring size. At 115 200 baud the UART drains ~11.5 bytes/ms.
const RING: usize = 2048;
/// History size.
const HIST: usize = 4096;
/// Longest log line; the rest of a longer one is cut off.
const LINE: usize = 256;
/// TX FIFO depth of the HPM UART.
const FIFO: usize = 16;

struct Console {
    ring: [u8; RING],
    head: usize,
    len: usize,
    /// Lines dropped because the ring was full.
    dropped: u32,
    hist: [u8; HIST],
    /// Bytes ever logged; byte `n` lives at `hist[n % HIST]` while `n + HIST > total`.
    total: u32,
}

static CONSOLE: Mutex<RefCell<Console>> =
    Mutex::new(RefCell::new(Console { ring: [0; RING], head: 0, len: 0, dropped: 0, hist: [0; HIST], total: 0 }));

/// While true (boot, until the executor runs) every `log!` is written out synchronously, so a
/// hang during init still leaves its last line on the UART.
static SYNC: AtomicBool = AtomicBool::new(true);

/// Switch from synchronous boot logging to the ring drained by [`drain`].
pub fn go_async() {
    SYNC.store(false, Relaxed);
}

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

/// Copy `src` into the circular buffer `buf` starting at index `at`.
fn put_wrapped(buf: &mut [u8], at: usize, src: &[u8]) {
    let first = src.len().min(buf.len() - at);
    buf[at..at + first].copy_from_slice(&src[..first]);
    buf[..src.len() - first].copy_from_slice(&src[first..]);
}

/// Log formatted text (the backend of `log!`).
pub fn write_fmt(args: fmt::Arguments) {
    struct Line {
        buf: [u8; LINE],
        len: usize,
    }
    impl Write for Line {
        fn write_str(&mut self, s: &str) -> fmt::Result {
            let n = s.len().min(LINE - self.len);
            self.buf[self.len..self.len + n].copy_from_slice(&s.as_bytes()[..n]);
            self.len += n;
            Ok(())
        }
    }
    // Formatting happens here, outside the critical section: with interrupts off it would
    // delay the ADC capture.
    let mut line = Line { buf: [0; LINE], len: 0 };
    let _ = line.write_fmt(args);
    if line.len == LINE {
        line.buf[LINE - 2..].copy_from_slice(b"\r\n"); // cut off: keep the line end
    }
    let text = &line.buf[..line.len];
    critical_section::with(|cs| {
        let c = &mut *CONSOLE.borrow_ref_mut(cs);
        if text.len() <= RING - c.len {
            put_wrapped(&mut c.ring, (c.head + c.len) % RING, text);
            c.len += text.len();
        } else {
            c.dropped = c.dropped.wrapping_add(1);
        }
        // The history gets the line even when the UART ring was full.
        put_wrapped(&mut c.hist, c.total as usize % HIST, text);
        c.total = c.total.wrapping_add(text.len() as u32);
    });
    if SYNC.load(Relaxed) {
        flush_blocking();
    }
}

/// Copy history bytes from position `from` on into `out`; returns (bytes ever logged, position
/// of the first byte copied, bytes copied). Positions older than the kept window are skipped.
pub fn history(from: u32, out: &mut [u8]) -> (u32, u32, usize) {
    critical_section::with(|cs| {
        let c = CONSOLE.borrow_ref(cs);
        let start = from.clamp(c.total.saturating_sub(HIST as u32), c.total);
        let n = ((c.total - start) as usize).min(out.len());
        let at = start as usize % HIST;
        let first = n.min(HIST - at);
        out[..first].copy_from_slice(&c.hist[at..at + first]);
        out[first..n].copy_from_slice(&c.hist[..n - first]);
        (c.total, start, n)
    })
}

/// Move up to one FIFO's worth of bytes to the UART. Call often (≥ every 1 ms).
pub fn drain() {
    if !tx_empty() {
        return;
    }
    critical_section::with(|cs| {
        let mut r = CONSOLE.borrow_ref_mut(cs);
        let n = r.len.min(FIFO);
        for _ in 0..n {
            let b = r.ring[r.head];
            pac::UART0.thr().write(|w| w.0 = b as u32);
            r.head = (r.head + 1) % RING;
            r.len -= 1;
        }
    });
}

/// Number of lines dropped because the UART ring was full.
pub fn dropped() -> u32 {
    critical_section::with(|cs| CONSOLE.borrow_ref(cs).dropped)
}

/// Synchronous output (boot, panic): flush the ring, then `s`, spinning on the FIFO.
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
            let mut r = CONSOLE.borrow_ref_mut(cs);
            if r.len == 0 {
                return None;
            }
            let b = r.ring[r.head];
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

/// `log!("fmt", args…)` — one line on the console (CRLF appended).
#[macro_export]
macro_rules! log {
    ($($arg:tt)*) => {
        $crate::console::write_fmt(format_args!("{}\r\n", format_args!($($arg)*)))
    };
}
