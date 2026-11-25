//! The one `Keyboard`, shared between the scan-processing interrupt and the thread-mode tasks.

use core::cell::RefCell;

use embassy_sync::blocking_mutex::Mutex;
use zent_core::device::Keyboard;

use crate::plic::ScanLock;

/// Initialised at link time (`Keyboard::new` is `const`): the ~40 KB struct is copied into DLM
/// by the startup code with `.data` and is never built on the 16 KB stack.
static KB: Mutex<ScanLock, RefCell<Keyboard>> = Mutex::new(RefCell::new(Keyboard::new()));

/// Run `f` on the keyboard with the scan-processing interrupt masked (see [`ScanLock`]).
/// Keep `f` short and never call `with` from inside it.
pub fn with<R>(f: impl FnOnce(&mut Keyboard) -> R) -> R {
    KB.lock(|kb| f(&mut kb.borrow_mut()))
}
