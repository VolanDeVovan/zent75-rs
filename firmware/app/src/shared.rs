//! Interrupt priorities and the two pieces of state shared between contexts: the `Keyboard`
//! (scan ISR ↔ thread tasks) and the `NorStorage` (thread tasks only).
//!
//! ```text
//! PLIC prio 7  ADC0 seq_cvc      per-step sample capture (deadline ~7 µs, never masked)
//! PLIC prio 3  USB0              hpm-hal driver, wakes tasks (short)
//! PLIC prio 2  GPTMR1            Keyboard::scan for each complete 88-sample frame; re-enables
//!                                MIE so ADC0/USB0 preempt it (PLIC preemptive mode)
//! core MTIP    MCHTMR            embassy time driver
//! thread       executor          USB tasks, vendor protocol, lighting, service(), flash
//! ```
//!
//! The `Keyboard` is locked from thread mode by raising the PLIC threshold to [`PRIO_SCAN`]:
//! this masks only the GPTMR1 scan-processing interrupt (and anything lower), never the ADC
//! capture, so taking the lock cannot corrupt samples. It is *not* a global critical section.

use core::cell::UnsafeCell;
use core::mem::MaybeUninit;

use hpm_hal::interrupt::{InterruptExt, Priority};
use hpm_hal::pac;
use zent_core::device::Keyboard;

use crate::flash::NorStorage;

pub const PRIO_ADC: Priority = Priority::P7;
pub const PRIO_USB: Priority = Priority::P3;
pub const PRIO_SCAN: Priority = Priority::P2;

struct KbCell(UnsafeCell<MaybeUninit<Keyboard>>);
// SAFETY: access is serialised by the PLIC-threshold protocol described above.
unsafe impl Sync for KbCell {}
static KB: KbCell = KbCell(UnsafeCell::new(MaybeUninit::uninit()));

/// Install the keyboard. Call once, before the scan interrupt is enabled.
pub fn kb_init(kb: Keyboard) {
    unsafe { (*KB.0.get()).write(kb) };
}

/// Thread-mode access: masks the scan-processing interrupt for the duration of `f`.
/// Keep `f` short (no flash, no awaits) — every µs here delays `Keyboard::scan`.
pub fn with_kb<R>(f: impl FnOnce(&mut Keyboard) -> R) -> R {
    let plic = pac::PLIC.targetconfig(0);
    // Raise the threshold with MIE off so an interrupt that is already being signalled
    // cannot slip in between; read back to make sure the PLIC took it.
    let prev = critical_section::with(|_| {
        let prev = plic.threshold().read().threshold();
        plic.threshold().write(|w| w.set_threshold(PRIO_SCAN as u32));
        let _ = plic.threshold().read();
        prev
    });
    // SAFETY: the scan ISR (the only other user) cannot run while the threshold is ≥ PRIO_SCAN,
    // and thread-mode callers never nest (with_kb is not re-entered from `f`).
    let r = f(unsafe { (*KB.0.get()).assume_init_mut() });
    plic.threshold().write(|w| w.set_threshold(prev));
    r
}

/// Interrupt-context access for the scan-processing ISR (priority [`PRIO_SCAN`]).
///
/// # Safety
/// Only from the GPTMR1 handler.
pub unsafe fn kb_from_scan_isr() -> &'static mut Keyboard {
    (*KB.0.get()).assume_init_mut()
}

/// Thread-mode-only cell (RefCell semantics without `Sync` requirements on the executor).
pub struct ThreadCell<T>(core::cell::RefCell<T>);
// SAFETY: only used from the single thread-mode executor (never from interrupts).
unsafe impl<T> Sync for ThreadCell<T> {}
impl<T> ThreadCell<T> {
    pub const fn new(v: T) -> Self {
        Self(core::cell::RefCell::new(v))
    }
    pub fn borrow_mut(&self) -> core::cell::RefMut<'_, T> {
        self.0.borrow_mut()
    }
}

pub static STORAGE: ThreadCell<NorStorage> = ThreadCell::new(NorStorage::new());

/// Enable PLIC preemptive-priority mode (stock: `PLIC.feature = 3`) and set priorities.
pub fn setup_plic() {
    pac::PLIC.feature().modify(|w| {
        w.set_preempt(true);
        w.set_vectored(true);
    });
    // hpm-hal's own HDMA handler would clear our RGB channel's status; we poll the channel.
    hpm_hal::interrupt::HDMA.disable();
}

pub fn set_prio(irq: impl InterruptExt, prio: Priority) {
    irq.set_priority(prio);
}
