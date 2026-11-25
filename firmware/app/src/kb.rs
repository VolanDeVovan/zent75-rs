//! The one `Keyboard`, shared between the scan-processing interrupt and the thread-mode tasks.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

use embassy_sync::blocking_mutex::Mutex;
use zent_core::device::Keyboard;
use zent_core::engine::curves::HeightTable;

use crate::plic::ScanLock;

/// Initialised at link time (`Keyboard::new` is `const`): the ~40 KB struct is copied into DLM
/// by the startup code with `.data` and is never built on the 16 KB stack.
static KB: Mutex<ScanLock, RefCell<Keyboard>> = Mutex::new(RefCell::new(Keyboard::new()));

/// Run `f` on the keyboard with the scan-processing interrupt masked (see [`ScanLock`]).
/// Keep `f` short and never call `with` from inside it.
pub fn with<R>(f: impl FnOnce(&mut Keyboard) -> R) -> R {
    KB.lock(|kb| f(&mut kb.borrow_mut()))
}

/// Precomputed height tables for the engine (one per switch type in use; a fifth type in use
/// is interpolated on the fly). 4 × 5.3 KB: too big for DLM, so in AHB SRAM, which the startup
/// code neither loads nor zeroes.
const HEIGHT_TABLES: usize = 4;
// SAFETY: `.ahb_sram` is a NOLOAD RAM section of `zent-link.x`; the contents are uninitialised
// until `attach_height_tables` zeroes them, hence `MaybeUninit`.
#[unsafe(link_section = ".ahb_sram")]
static mut TABLES: MaybeUninit<[HeightTable; HEIGHT_TABLES]> = MaybeUninit::uninit();
static TABLES_TAKEN: AtomicBool = AtomicBool::new(false);

/// Give the keyboard its height-table storage. Call once, before `Keyboard::boot`.
pub fn attach_height_tables() {
    assert!(!TABLES_TAKEN.swap(true, Relaxed), "height tables attached twice");
    // SAFETY: `TABLES` is only ever touched here, and the flag above lets this run once, so the
    // `&'static mut` is unique. Zeroing first makes the (integer-only) contents initialised
    // without building the 21 KB value on the stack.
    let tables: &'static mut [HeightTable] = unsafe {
        let p = &raw mut TABLES;
        core::ptr::write_bytes((*p).as_mut_ptr(), 0, 1);
        (*p).assume_init_mut()
    };
    with(|kb| kb.set_height_tables(tables));
}
