//! PLIC set-up: interrupt priorities, the hpm-hal workarounds, and the lock that keeps the
//! scan-processing interrupt away from shared state.
//!
//! ```text
//! PLIC prio 7  ADC0 seq_cvc      per-step sample capture (deadline ~7 µs, never masked)
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

/// Enable PLIC preemptive-priority mode (stock: `PLIC.feature = 3`).
pub fn setup() {
    pac::PLIC.feature().modify(|w| {
        w.set_preempt(true);
        w.set_vectored(true);
    });
    // hpm-hal's own HDMA handler would clear our RGB channel's status; we poll the channel.
    hpm_hal::interrupt::HDMA.disable();
}

/// Complete (end of service) a PLIC source with a plain *write* of the claim/complete register.
/// Never use hpm-hal's `InterruptExt::complete()`: it is a read-modify-write, and reading that
/// register claims the highest-priority pending source — e.g. finishing GPTMR1 (prio 2) while
/// an ADC0 step (prio 7) is pending silently claims ADC0, which is then never serviced or
/// completed, so the PLIC masks every source ≤ 7 forever and the scan stops.
#[inline(always)]
pub fn complete(irq: impl InterruptExt) {
    pac::PLIC.targetconfig(0).claim().write(|w| w.set_interrupt_id(irq.number() as u16));
}

/// Set a PLIC source priority. Not `InterruptExt::set_priority`: hpm-hal 620f58d passes the IRQ
/// number to metapac's `PLIC.priority(n)`, whose index 0 is *source 1* (register 0x04), so it
/// programs the priority of the next source (ADC0 → ADC1's register) and leaves ours at its
/// reset value. Here the register of source `n` is `PLIC + 4·n`, as in hpm_sdk.
pub fn set_priority(irq: impl InterruptExt, prio: Priority) {
    pac::PLIC.priority(irq.number() - 1).write(|w| w.set_priority(prio as u32));
}

/// Give `irq` its priority and unmask it. Call once its handler's state is ready.
pub fn enable(irq: impl InterruptExt, prio: Priority) {
    set_priority(irq, prio);
    // SAFETY: unmasking a source is only unsafe in that its handler starts to run. Every
    // handler in this firmware (`scan::ADC0`, `scan::GPTMR1`, `usb::USB0`) works on statics
    // that are valid from reset and shares state only through atomics and `ScanLock`.
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
