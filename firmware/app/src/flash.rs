//! The 1 MB XPI0 NOR behind `zent_core::storage::nor::BankStore`, plus the OTP chip UID.
//!
//! [`RomNor`] is the raw device: erase and program go through hpm-hal's `flash::Flash` (the
//! boot-ROM NOR driver, same calls and option words as stock 0.0.18, docs/re-peripherals.md §6,
//! §8), reads come from the memory-mapped XIP window. What is stored where, staging and
//! verification live in the core's `BankStore`.
//!
//! **Write range.** `BankStore` refuses every erase/program outside `0x80070000..0x80100000`
//! before it gets here; [`RomNor`] checks `check_write_range` again right in front of the ROM
//! call. The boot region `0x80000000..0x8003FFFF` and the app image can therefore never be
//! touched by this firmware.
//!
//! **Concurrency.** All code and read-only data run from ILM/DLM (`zent-link.x`), so flash
//! operations run with interrupts *enabled*: the scan interrupts and USB keep working while a
//! sector is erased. Flash is only used from thread mode; the executor is blocked for the
//! duration of one ROM call (sector erase typ. 45 ms, page program < 1 ms), which pauses USB
//! report *sending* but not key scanning.

use core::sync::atomic::{AtomicU32, Ordering};

use hpm_hal::flash::{Config, Flash};
use hpm_hal::peripherals::XPI0;
use hpm_hal::Peri;
use zent_core::storage::check_write_range;
use zent_core::storage::nor::{BankStore, DeviceError, Nor, PAGE_SIZE, SECTOR_SIZE};
use zent_core::storage::{FLASH_WRITABLE_END, FLASH_WRITABLE_START};

use crate::log;

/// The config-bank storage of the board.
pub type Store = BankStore<RomNor>;

const FLASH_BASE: u32 = 0x8000_0000;
const FLASH_SIZE: usize = 0x10_0000;
/// Stock `auto_config` option (`{0xFCF90002, 0x00000006, 0x00001000}`).
const NOR_OPTION: Config = Config { header: 0xFCF9_0002, option0: 0x0000_0006, option1: 0x0000_1000 };

/// Erase/program requests stopped in front of the ROM call (should stay 0 forever; logged).
pub static REFUSED: AtomicU32 = AtomicU32::new(0);

pub struct RomNor {
    /// `None` until [`RomNor::init`] succeeded: config writes are disabled, reads still work.
    flash: Option<Flash<'static, XPI0, FLASH_SIZE>>,
}

impl RomNor {
    pub const fn new() -> Self {
        Self { flash: None }
    }

    /// ROM `auto_config` + geometry check. Must run before the first erase/program.
    pub fn init(&mut self, xpi: Peri<'static, XPI0>) -> Result<(), hpm_hal::flash::Error> {
        // the driver only accepts a 1 MB part and reports the ROM's sector size to itself; the
        // core's layout assumes the 4 KB sectors of this part
        const _: () = assert!(hpm_hal::flash::ERASE_SIZE == SECTOR_SIZE as usize);
        const _: () = assert!(hpm_hal::flash::PAGE_SIZE == PAGE_SIZE as usize);
        self.flash = Some(Flash::new(xpi, NOR_OPTION)?);
        xip_changed();
        Ok(())
    }

    /// Last check in front of the ROM: `[addr, addr + len)` must be config area.
    fn writable(addr: u32, len: u32) -> bool {
        let ok = check_write_range(addr, len);
        if !ok {
            REFUSED.fetch_add(1, Ordering::Relaxed);
            log!("flash: REFUSED write 0x{:08x}+0x{:x}", addr, len);
        }
        ok
    }

    fn done(what: &str, addr: u32, res: Result<(), hpm_hal::flash::Error>) -> Result<(), DeviceError> {
        xip_changed();
        res.map_err(|e| {
            log!("flash: {} 0x{:08x} FAILED {:?}", what, addr, e);
            DeviceError
        })
    }
}

impl Nor for RomNor {
    fn read(&mut self, addr: u32, out: &mut [u8]) {
        xip_read(addr, out);
    }

    fn erase_sector(&mut self, addr: u32) -> Result<(), DeviceError> {
        let flash = self.flash.as_mut().ok_or(DeviceError)?;
        if !Self::writable(addr, SECTOR_SIZE) {
            return Err(DeviceError);
        }
        crate::system::watchdog_feed(); // the longest stall this firmware has
        let off = addr - FLASH_BASE;
        Self::done("erase", addr, flash.blocking_erase(off, off + SECTOR_SIZE))
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), DeviceError> {
        /// The ROM takes its source as a word pointer.
        #[repr(C, align(4))]
        struct Page([u8; PAGE_SIZE as usize]);

        let flash = self.flash.as_mut().ok_or(DeviceError)?;
        if data.len() > PAGE_SIZE as usize || !Self::writable(addr, data.len() as u32) {
            return Err(DeviceError);
        }
        let mut page = Page([0xFF; PAGE_SIZE as usize]);
        page.0[..data.len()].copy_from_slice(data);
        Self::done("program", addr, flash.blocking_write(addr - FLASH_BASE, &page.0[..data.len()]))
    }
}

/// Copy config-area bytes out of the memory-mapped NOR.
fn xip_read(addr: u32, out: &mut [u8]) {
    let end = addr.checked_add(out.len() as u32);
    assert!(addr >= FLASH_WRITABLE_START && end.is_some_and(|e| e <= FLASH_WRITABLE_END));
    // SAFETY: the range is inside the XIP window of the 1 MB NOR, which is always mapped and
    // readable; `out` is a distinct RAM buffer. Nothing else accesses the window meanwhile:
    // flash is only used from thread mode and the ROM calls that rewrite it are synchronous.
    unsafe { core::ptr::copy_nonoverlapping(addr as *const u8, out.as_mut_ptr(), out.len()) }
}

/// The NOR changed underneath the XIP window (or may have): drop stale lines from the L1
/// caches so later reads see the new content.
///
/// Whole-D-cache "write back + invalidate": the only cacheable memory this firmware touches is
/// the XIP window (ILM/DLM are not cached, peripherals are device memory), so nothing dirty
/// can be lost, and the command does not depend on the cache-line size CSR (andes-riscv's
/// per-line helpers panic when `mdcm_cfg` reads 0).
fn xip_changed() {
    riscv::asm::fence_i();
    // SAFETY: a write of `L1D_WBINVAL_ALL` to the Andes `mcctlcommand` CSR; it only discards
    // cache state that is re-read from memory on the next access.
    unsafe { andes_riscv::l1c::dc_flush_all() };
}

// ------------------------------------------------------------------ OTP UID

/// Start of the boot-ROM API table: a list of pointers to driver interfaces.
const ROM_API_TABLE: usize = 0x2001_FF00;
/// Index of `otp_driver_if` in that table (hpm_sdk `bootloader_api_table_t`).
const ROM_API_OTP: usize = 3;

/// Head of the ROM's `otp_driver_interface_t` (hpm_sdk `hpm_romapi.h`).
#[repr(C)]
struct OtpDriver {
    version: u32,
    init: unsafe extern "C" fn(),
    deinit: unsafe extern "C" fn(),
    read_from_shadow: unsafe extern "C" fn(addr: u32) -> u32,
}

/// 128-bit chip UID: OTP shadow words 0x58..0x5B via the ROM OTP driver (stock `BE 1D`).
/// hpm-hal has no OTP driver, so this is the one ROM call made directly.
pub fn read_uid() -> [u8; 16] {
    let mut out = [0u8; 16];
    // SAFETY: the mask ROM is always mapped; entry 3 of its API table points to the OTP driver
    // interface, whose first members have the layout of `OtpDriver` on every HPM5300 ROM. Both
    // functions only read the OTP shadow registers.
    unsafe {
        let otp = &**((ROM_API_TABLE as *const *const OtpDriver).add(ROM_API_OTP));
        (otp.init)();
        for (word, bytes) in (0x58u32..0x5C).zip(out.as_chunks_mut::<4>().0) {
            *bytes = (otp.read_from_shadow)(word).to_le_bytes();
        }
    }
    out
}
