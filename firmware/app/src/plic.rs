//! PLIC set-up: interrupt priorities and the lock that keeps the scan-processing interrupt away
//! from shared state.
//!
//! ```text
//! PLIC prio 7  ADC0 seq_cvc      per-step sample capture (deadline ~7 µs, never masked)
//! PLIC prio 5  GPTMR0            profiler sample (feature `profiler`, only during a capture)
//! PLIC prio 3  USB0              hpm-hal driver, wakes tasks (short)
//! PLIC prio 2  GPTMR1            Keyboard::scan for each complete 88-sample frame; re-enables
//!                                MIE so ADC0/USB0 preempt it (PLIC preemptive mode)
//! core MTIP    MCHTMR            embassy time driver
//! thread       executor          USB tasks, vendor protocol, lighting, service(), flash
//! ```

use embassy_sync::blocking_mutex::raw::RawMutex;
use hpm_hal::interrupt::{InterruptExt, Priority};
use hpm_hal::pac;

pub const PRIO_ADC: Priority = Priority::P7;
pub const PRIO_USB: Priority = Priority::P3;
pub const PRIO_SCAN: Priority = Priority::P2;
/// Profiler sampling timer (`debug::prof`): above everything it should see, below the ADC
/// capture.
#[cfg(feature = "profiler")]
pub const PRIO_PROF: Priority = Priority::P5;

/// Enable PLIC preemptive-priority mode (stock: `PLIC.feature = 3`).
pub fn setup() {
    pac::PLIC.feature().modify(|w| {
        w.set_preempt(true);
        w.set_vectored(true);
    });
    // hpm-hal's own HDMA handler would clear our RGB channel's status; we poll the channel.
    hpm_hal::interrupt::HDMA.disable();
}

/// Give `irq` its priority and unmask it. Call once its handler's state is ready.
pub fn enable(irq: impl InterruptExt, prio: Priority) {
    irq.set_priority(prio);
    // SAFETY: unmasking a source is only unsafe in that its handler starts to run. The handlers
    // enabled through here (`scan::ADC0`, `scan::GPTMR1`, `debug::prof::GPTMR0`) work on
    // statics that are valid from reset and share state only through atomics and `ScanLock`.
    unsafe { irq.enable() };
}

/// Raw mutex for state shared between thread mode and the scan-processing interrupt
/// (GPTMR1, [`PRIO_SCAN`]): holding it raises the PLIC threshold to `PRIO_SCAN`, which masks
/// that interrupt (and anything lower) but never the ADC capture or USB — it is *not* a global
/// critical section, so taking it cannot cost samples.
///
/// Keep the locked section short (no flash, no awaits): every µs delays `Keyboard::scan`.
pub struct ScanLock;

// SAFETY: `RawMutex::lock` must give the closure exclusive access with respect to every other
// context that locks the same mutex. The contexts are thread mode (one executor, single hart)
// and the GPTMR1 handler; USB0 and ADC0 run at higher priorities and never take it (they only
// touch atomics and wake tasks). While thread mode holds the lock the threshold is PRIO_SCAN,
// so GPTMR1 cannot be delivered; GPTMR1 itself is never preempted by thread mode, and inside
// its handler the PLIC has already raised the threshold to its priority. Nested locking from
// one context is caught by the `RefCell` every `ScanLock` mutex in this firmware wraps.
unsafe impl RawMutex for ScanLock {
    const INIT: Self = ScanLock;

    fn lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let plic = pac::PLIC.targetconfig(0);
        // Raise the threshold with MIE off so an interrupt that is already being signalled
        // cannot slip in between; read back to make sure the PLIC took it.
        let prev = critical_section::with(|_| {
            let prev = plic.threshold().read().threshold();
            plic.threshold().write(|w| w.set_threshold(prev.max(PRIO_SCAN as u32)));
            let _ = plic.threshold().read();
            prev
        });
        let r = f();
        plic.threshold().write(|w| w.set_threshold(prev));
        r
    }
}
