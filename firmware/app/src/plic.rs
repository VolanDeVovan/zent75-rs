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

/// Raw mutex for state shared between thread mode and the scan-processing interrupt (GPTMR1):
/// holding it clears GPTMR1's *enable* bit in the PLIC, so that one handler cannot start, while
/// the ADC capture, USB and everything else keep running — it is not a global critical section,
/// so taking it cannot cost samples. A GPTMR1 request raised meanwhile stays pending in the
/// gateway and is delivered as soon as the bit is set again.
///
/// It deliberately does not touch the PLIC threshold: in preemptive mode the hardware raises and
/// restores the threshold itself on every claim / completion, and software writes racing with
/// that left the threshold stuck at the scan priority on hardware (scan stopped, keys stuck).
///
/// Keep the locked section short (no flash, no awaits): every µs delays `Keyboard::scan`.
pub struct ScanLock;

const SCAN_IRQ: u32 = pac::interrupt::GPTMR1 as u32;

fn scan_irq_enabled(on: bool) -> bool {
    let reg = pac::PLIC.targetint(0).inten((SCAN_IRQ / 32) as usize);
    let bit = 1 << (SCAN_IRQ % 32);
    let was = reg.read().0 & bit != 0;
    reg.modify(|w| if on { w.0 |= bit } else { w.0 &= !bit });
    was
}

// SAFETY: `RawMutex::lock` must give the closure exclusive access with respect to every other
// context that locks the same mutex. Those are thread mode (one executor, single hart) and the
// GPTMR1 handler; ADC0, USB0 and GPTMR0 never take it (they only touch atomics and wake tasks).
// While thread mode holds the lock GPTMR1 is disabled in the PLIC, so its handler cannot run;
// the handler itself is never preempted by thread mode. The enable register is only modified
// with MIE off (here and at init), so the read-modify-write cannot race. Nested locking from one
// context is caught by the `RefCell` every `ScanLock` mutex in this firmware wraps.
unsafe impl RawMutex for ScanLock {
    const INIT: Self = ScanLock;

    fn lock<R>(&self, f: impl FnOnce() -> R) -> R {
        let was = critical_section::with(|_| scan_irq_enabled(false));
        let r = f();
        if was {
            critical_section::with(|_| scan_irq_enabled(true));
        }
        r
    }
}
