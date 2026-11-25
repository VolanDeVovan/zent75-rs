//! Config-bank storage in the 1 MB XPI0 NOR through the boot-ROM driver, plus the OTP chip UID.
//!
//! Same layout and ROM calls as stock 0.0.18 (docs/re-peripherals.md §6, §8):
//!
//! | Slot        | Flash                      | Stock writes | We keep            |
//! |-------------|----------------------------|--------------|--------------------|
//! | bank 1      | `0x80070000` (1 sector)    | `0x790` B    | whole 4 KB sector  |
//! | profile k   | `0x80080000 + k·0x2000`    | `0x1E38` B   | whole 8 KB slot    |
//!
//! **Safety.** [`nor_modify`] is the only function that obtains the ROM erase/program entry
//! points. It calls `zent_core::storage::check_write_range` and refuses (returns
//! [`FlashError::Refused`]) anything outside `0x80070000..0x80100000`, plus anything not
//! sector-aligned for erases. The boot region `0x80000000..0x8003FFFF` and the app image can
//! therefore never be touched by this firmware.
//!
//! **Concurrency.** All code and read-only data run from ILM/DLM (`zent-link.x`), so flash
//! operations run with interrupts *enabled*: the scan ISRs and USB keep working while a sector
//! is erased. Flash ops are only issued from thread mode (the storage task); the thread
//! executor is blocked for the duration of one ROM call (sector erase typ. 45 ms, page program
//! < 1 ms), which pauses USB report *sending* but not key scanning.
//!
//! **Staging.** [`NorStorage`] reads straight from the memory-mapped NOR. `store` stages a
//! changed bank-1 or profile image in RAM and marks it dirty (cheap, can run under the keyboard
//! lock); [`NorStorage::commit`] writes it later from the low-priority task (debounced, and
//! forced before reset / bootloader). XIP is only read from that same thread context, never
//! while a ROM erase/program runs.

use core::sync::atomic::{AtomicU32, Ordering};

use zent_core::storage::{check_write_range, Slot, Storage, StorageError};

use crate::log;

const ROM_API_TABLE: usize = 0x2001_FF00;
const XPI0_BASE: usize = 0xF300_0000;
const XPI_CHANNEL_AUTO: u32 = 4;
const FLASH_BASE: u32 = 0x8000_0000;
/// Stock `auto_config` option (`{0xFCF90002, 0x00000006, 0x00001000}`).
const NOR_OPTION: [u32; 4] = [0xFCF9_0002, 0x0000_0006, 0x0000_1000, 0];
const PROP_TOTAL_SIZE: u32 = 0;
const PROP_PAGE_SIZE: u32 = 1;
const PROP_SECTOR_SIZE: u32 = 2;

pub const SECTOR: u32 = 4096;
const PAGE: u32 = 256;

pub const BANK1_ADDR: u32 = 0x8007_0000;
pub const PROFILE_ADDR: u32 = 0x8008_0000;
pub const PROFILE_STRIDE: u32 = 0x2000;
const PROFILES: usize = zent_core::layout::PROFILE_COUNT;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlashError {
    /// Outside the config area or misaligned — refused before any ROM call.
    Refused,
    /// ROM driver returned a non-zero status.
    Rom(u32),
    /// Read-back after programming differs.
    Verify,
    /// `init` has not run / failed.
    NotReady,
}

/// Operations refused by the range guard (should stay 0 forever; logged).
pub static REFUSED: AtomicU32 = AtomicU32::new(0);

// ------------------------------------------------------------------ ROM API

/// `xpi_nor_config_t` is 256 bytes; keep generous room for ROM variants.
#[repr(C, align(8))]
struct NorConfig([u32; 128]);

static mut NOR_CFG: NorConfig = NorConfig([0; 128]);
static mut NOR_READY: bool = false;

type FnAutoConfig = unsafe extern "C" fn(base: usize, cfg: *mut NorConfig, opt: *const [u32; 4]) -> u32;
type FnGetProperty = unsafe extern "C" fn(base: usize, cfg: *const NorConfig, id: u32, out: *mut u32) -> u32;
type FnErase = unsafe extern "C" fn(base: usize, ch: u32, cfg: *const NorConfig, start: u32, len: u32) -> u32;
type FnProgram =
    unsafe extern "C" fn(base: usize, ch: u32, cfg: *const NorConfig, src: *const u32, dst: u32, len: u32) -> u32;

/// Read a function pointer from a ROM driver interface (`table_off` in the API table, `off` in
/// the interface). Offsets: docs/re-peripherals.md §6 / hpm_sdk `hpm_romapi.h`.
unsafe fn rom_fn(table_off: usize, off: usize) -> usize {
    let iface = core::ptr::read_volatile((ROM_API_TABLE + table_off) as *const usize);
    core::ptr::read_volatile((iface + off) as *const usize)
}
const NOR_IF: usize = 0x14;
const OTP_IF: usize = 0x0C;

/// `auto_config` + properties. Must run before any other flash call.
pub fn init() -> Result<(u32, u32, u32), FlashError> {
    unsafe {
        let auto: FnAutoConfig = core::mem::transmute(rom_fn(NOR_IF, 0x4C));
        let get: FnGetProperty = core::mem::transmute(rom_fn(NOR_IF, 0x50));
        let cfg = &raw mut NOR_CFG;
        let st = auto(XPI0_BASE, cfg, &NOR_OPTION);
        if st != 0 {
            return Err(FlashError::Rom(st));
        }
        let (mut total, mut sector, mut page) = (0u32, 0u32, 0u32);
        get(XPI0_BASE, cfg, PROP_TOTAL_SIZE, &mut total);
        get(XPI0_BASE, cfg, PROP_SECTOR_SIZE, &mut sector);
        get(XPI0_BASE, cfg, PROP_PAGE_SIZE, &mut page);
        if sector != SECTOR || total < 0x10_0000 {
            log!("flash: unexpected geometry total {} sector {} page {}", total, sector, page);
            return Err(FlashError::NotReady);
        }
        NOR_READY = true;
        Ok((total, sector, page))
    }
}

enum Op<'a> {
    EraseSector,
    Program(&'a [u32]),
}

/// THE single flash write path. Every erase and program in this firmware goes through here.
fn nor_modify(addr: u32, len: u32, op: Op) -> Result<(), FlashError> {
    let aligned = match op {
        Op::EraseSector => len == SECTOR && addr.is_multiple_of(SECTOR),
        Op::Program(src) => len as usize == src.len() * 4 && len > 0 && len <= PAGE && addr % PAGE + len <= PAGE,
    };
    if !aligned || !check_write_range(addr, len) {
        REFUSED.fetch_add(1, Ordering::Relaxed);
        log!("flash: REFUSED write 0x{:08x}+0x{:x}", addr, len);
        return Err(FlashError::Refused);
    }
    // SAFETY: NOR_READY/NOR_CFG are written once by `init`; ROM entry points are fixed.
    unsafe {
        if !NOR_READY {
            return Err(FlashError::NotReady);
        }
        let cfg = &raw const NOR_CFG;
        let off = addr - FLASH_BASE;
        let st = match op {
            Op::EraseSector => {
                let erase: FnErase = core::mem::transmute(rom_fn(NOR_IF, 0x18));
                erase(XPI0_BASE, XPI_CHANNEL_AUTO, cfg, off, len)
            }
            Op::Program(src) => {
                let program: FnProgram = core::mem::transmute(rom_fn(NOR_IF, 0x28));
                program(XPI0_BASE, XPI_CHANNEL_AUTO, cfg, src.as_ptr(), off, len)
            }
        };
        core::arch::asm!("fence.i");
        dcache_drop_xip();
        if st != 0 {
            return Err(FlashError::Rom(st));
        }
    }
    Ok(())
}

/// Drop stale XIP lines from the L1 D-cache after the NOR changed underneath it.
///
/// Whole-cache "write back + invalidate" (Andes CCTL `L1D_WBINVAL_ALL`): the only cacheable
/// memory this firmware touches is the XIP window (ILM/DLM are not cached, peripherals are
/// device memory), so nothing dirty can be lost, and the command does not depend on the
/// cache-line size CSR (andes-riscv's per-line helper panics when `mdcm_cfg` reads 0).
fn dcache_drop_xip() {
    const L1D_WBINVAL_ALL: usize = 6;
    unsafe { core::arch::asm!("csrw 0x7CC, {0}", in(reg) L1D_WBINVAL_ALL) };
}

/// Read through the XIP window (after dropping stale D-cache lines).
fn nor_read(addr: u32, out: &mut [u32]) {
    dcache_drop_xip();
    unsafe {
        for (i, w) in out.iter_mut().enumerate() {
            *w = core::ptr::read_volatile((addr as usize + i * 4) as *const u32);
        }
    }
}

/// Erase + program + verify `[addr, addr + data.len()*4)`, sector by sector. Feeds the watchdog
/// between sectors.
fn write_region(addr: u32, data: &[u32]) -> Result<(), FlashError> {
    let words_per_sector = SECTOR as usize / 4;
    for (s, chunk) in data.chunks(words_per_sector).enumerate() {
        let sector = addr + s as u32 * SECTOR;
        crate::system::watchdog_feed();
        nor_modify(sector, SECTOR, Op::EraseSector)?;
        for (p, page) in chunk.chunks(PAGE as usize / 4).enumerate() {
            // Skip all-0xFF pages: erased flash already reads 0xFF.
            if page.iter().all(|&w| w == u32::MAX) {
                continue;
            }
            nor_modify(sector + p as u32 * PAGE, (page.len() * 4) as u32, Op::Program(page))?;
        }
        let mut check = [0u32; 64];
        for (p, page) in chunk.chunks(64).enumerate() {
            let a = sector + p as u32 * 256;
            nor_read(a, &mut check[..page.len()]);
            if check[..page.len()] != *page {
                return Err(FlashError::Verify);
            }
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ OTP UID

/// 128-bit chip UID: OTP shadow words 0x58..0x5B via the ROM OTP driver (stock `BE 1D`).
pub fn read_uid() -> [u8; 16] {
    let mut out = [0u8; 16];
    unsafe {
        let init: unsafe extern "C" fn() = core::mem::transmute(rom_fn(OTP_IF, 0x04));
        let read: unsafe extern "C" fn(u32) -> u32 = core::mem::transmute(rom_fn(OTP_IF, 0x0C));
        init();
        for (i, w) in (0x58u32..0x5C).enumerate() {
            out[i * 4..i * 4 + 4].copy_from_slice(&read(w).to_le_bytes());
        }
    }
    out
}

// ------------------------------------------------------------------ Storage

/// Bytes of a slot the firmware owns (= stock shadow sizes); the rest of the slot's sectors is
/// erased on a write, like stock.
const BANK1_LEN: usize = zent_core::config::BANK1_SIZE;
const PROFILE_LEN: usize = zent_core::config::BANK2_SIZE;
const _: () = assert!(BANK1_LEN % 4 == 0 && PROFILE_LEN % 4 == 0);
const _: () = assert!(BANK1_LEN <= SECTOR as usize && PROFILE_LEN <= PROFILE_STRIDE as usize);

fn bytes(w: &[u32]) -> &[u8] {
    // SAFETY: u32 → u8 view of the same memory, alignment only decreases.
    unsafe { core::slice::from_raw_parts(w.as_ptr() as *const u8, w.len() * 4) }
}
fn bytes_mut(w: &mut [u32]) -> &mut [u8] {
    // SAFETY: as above.
    unsafe { core::slice::from_raw_parts_mut(w.as_mut_ptr() as *mut u8, w.len() * 4) }
}

/// Byte `i` of the NOR through the XIP window. Only from the storage/task context, never
/// while a ROM erase/program is in flight (all flash ops run synchronously in thread mode).
fn xip_byte(addr: u32) -> u8 {
    unsafe { core::ptr::read_volatile(addr as usize as *const u8) }
}

/// Decide whether a stored blob counts as "nothing stored". Stock 0.0.18 writes real banks
/// (non-uniform contents: calibration values, keymaps). An erased slot reads all 0xFF; an
/// all-zero slot is equally meaningless. Anything else is accepted as-is so the owner's
/// stock-written config is used unchanged.
fn is_empty(mut b: impl Iterator<Item = u8> + Clone) -> bool {
    b.clone().all(|x| x == 0xFF) || b.all(|x| x == 0x00)
}

/// Where the current content of a slot lives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Src {
    Flash,
    Staged,
    Blank,
}

/// Staged config writes. Reads come straight from the memory-mapped NOR (XIP); a `store` that
/// changes something is staged in RAM (cheap, runs under the keyboard lock) and written by
/// [`NorStorage::commit`] from the low-priority task (debounced, forced before reset /
/// bootloader). RAM: one bank-1 image and one profile image (stock sizes) — profiles are only
/// ever stored for the active one; an all-0xFF store (wipe) needs no buffer at all.
pub struct NorStorage {
    bank1: [u32; BANK1_LEN / 4],
    bank1_dirty: bool,
    profile: [u32; PROFILE_LEN / 4],
    /// Profile whose image is staged in `profile` (dirty).
    profile_slot: Option<u8>,
    /// Profiles to erase (bit k).
    blank: u8,
    /// Last `store` that changed something (ms), for debouncing.
    last_change_ms: u64,
    pub commits: u32,
    pub errors: u32,
}

impl NorStorage {
    pub const fn new() -> Self {
        Self {
            bank1: [0; BANK1_LEN / 4],
            bank1_dirty: false,
            profile: [0; PROFILE_LEN / 4],
            profile_slot: None,
            blank: 0,
            last_change_ms: 0,
            commits: 0,
            errors: 0,
        }
    }

    fn addr(slot: Slot) -> Option<(u32, usize)> {
        match slot {
            Slot::Global => Some((BANK1_ADDR, BANK1_LEN)),
            Slot::Profile(k) if (k as usize) < PROFILES => Some((PROFILE_ADDR + k as u32 * PROFILE_STRIDE, PROFILE_LEN)),
            Slot::Profile(_) => None,
        }
    }

    fn src(&self, slot: Slot) -> Src {
        match slot {
            Slot::Global if self.bank1_dirty => Src::Staged,
            Slot::Profile(k) if self.profile_slot == Some(k) => Src::Staged,
            Slot::Profile(k) if self.blank & (1 << k) != 0 => Src::Blank,
            _ => Src::Flash,
        }
    }

    /// Current content byte `i` of `slot` (staged, pending erase or flash).
    fn byte(&self, slot: Slot, i: usize) -> u8 {
        let (addr, _) = Self::addr(slot).unwrap_or((BANK1_ADDR, 0));
        match self.src(slot) {
            Src::Staged if slot == Slot::Global => bytes(&self.bank1)[i],
            Src::Staged => bytes(&self.profile)[i],
            Src::Blank => 0xFF,
            Src::Flash => xip_byte(addr + i as u32),
        }
    }

    pub fn dirty(&self) -> bool {
        self.bank1_dirty || self.profile_slot.is_some() || self.blank != 0
    }

    /// Write every dirty slot if `force`, or if the last change is older than `debounce_ms`.
    /// Returns the number of slots written. Call from thread mode only, never under the
    /// keyboard lock.
    pub fn commit(&mut self, now_ms: u64, debounce_ms: u64, force: bool) -> usize {
        if !self.dirty() || (!force && now_ms.saturating_sub(self.last_change_ms) < debounce_ms) {
            return 0;
        }
        let mut n = 0;
        if self.bank1_dirty {
            self.bank1_dirty = false;
            n += self.finish(Slot::Global, BANK1_ADDR, |s| write_region(BANK1_ADDR, &s.bank1));
        }
        if let Some(k) = self.profile_slot.take() {
            let addr = PROFILE_ADDR + k as u32 * PROFILE_STRIDE;
            n += self.finish(Slot::Profile(k), addr, |s| write_region(addr, &s.profile));
        }
        for k in 0..PROFILES as u8 {
            if self.blank & (1 << k) != 0 {
                self.blank &= !(1 << k);
                let addr = PROFILE_ADDR + k as u32 * PROFILE_STRIDE;
                n += self.finish(Slot::Profile(k), addr, |_| erase_region(addr, PROFILE_STRIDE));
            }
        }
        n
    }

    fn finish(&mut self, slot: Slot, addr: u32, write: impl Fn(&Self) -> Result<(), FlashError>) -> usize {
        let mut res = write(self);
        if res.is_err() {
            res = write(self); // one retry
        }
        match res {
            Ok(()) => {
                self.commits += 1;
                log!("flash: saved {:?} @0x{:08x}", slot, addr);
                1
            }
            Err(e) => {
                // The keyboard's RAM copy stays authoritative; do not retry forever.
                self.errors += 1;
                log!("flash: save {:?} @0x{:08x} FAILED {:?}", slot, addr, e);
                0
            }
        }
    }
}

/// Erase `len` bytes (whole sectors) and verify they read 0xFF.
fn erase_region(addr: u32, len: u32) -> Result<(), FlashError> {
    for s in (0..len).step_by(SECTOR as usize) {
        crate::system::watchdog_feed();
        nor_modify(addr + s, SECTOR, Op::EraseSector)?;
    }
    dcache_drop_xip();
    if (0..len).any(|i| xip_byte(addr + i) != 0xFF) {
        return Err(FlashError::Verify);
    }
    Ok(())
}

impl Storage for NorStorage {
    fn load(&mut self, slot: Slot, buf: &mut [u8]) -> Result<(), StorageError> {
        let (_, len) = Self::addr(slot).ok_or(StorageError::Io)?;
        let n = buf.len().min(len);
        dcache_drop_xip();
        if is_empty((0..n).map(|i| self.byte(slot, i))) {
            return Err(StorageError::Empty);
        }
        for (i, b) in buf[..n].iter_mut().enumerate() {
            *b = self.byte(slot, i);
        }
        Ok(())
    }

    fn store(&mut self, slot: Slot, data: &[u8]) -> Result<(), StorageError> {
        let (_, len) = Self::addr(slot).ok_or(StorageError::Io)?;
        let n = data.len().min(len);
        dcache_drop_xip();
        if data[..n].iter().enumerate().all(|(i, &b)| self.byte(slot, i) == b) {
            return Ok(()); // unchanged: no flash wear
        }
        self.last_change_ms = embassy_time::Instant::now().as_millis();
        match slot {
            Slot::Global => {
                if !self.bank1_dirty {
                    // partial store: keep the rest of the current content
                    for i in n..len {
                        bytes_mut(&mut self.bank1)[i] = xip_byte(BANK1_ADDR + i as u32);
                    }
                }
                bytes_mut(&mut self.bank1)[..n].copy_from_slice(&data[..n]);
                self.bank1_dirty = true;
            }
            Slot::Profile(k) => {
                if n == len && data.iter().all(|&b| b == 0xFF) {
                    // erase only (wipe): no image needed
                    if self.profile_slot == Some(k) {
                        self.profile_slot = None;
                    }
                    self.blank |= 1 << k;
                    return Ok(());
                }
                if let Some(j) = self.profile_slot.filter(|&j| j != k) {
                    // Only one profile image is staged. Two different profiles within one
                    // debounce window (save, then a profile switch that repairs the new one):
                    // flush the older one now, under the keyboard lock (stock always saves
                    // with interrupts masked, so a ~100 ms pause is no regression).
                    log!("flash: flushing profile {} early", j);
                    self.profile_slot = None;
                    let addr = PROFILE_ADDR + j as u32 * PROFILE_STRIDE;
                    self.finish(Slot::Profile(j), addr, |s| write_region(addr, &s.profile));
                }
                if self.profile_slot != Some(k) {
                    let from_blank = self.blank & (1 << k) != 0;
                    for i in n..len {
                        bytes_mut(&mut self.profile)[i] =
                            if from_blank { 0xFF } else { xip_byte(PROFILE_ADDR + k as u32 * PROFILE_STRIDE + i as u32) };
                    }
                }
                bytes_mut(&mut self.profile)[..n].copy_from_slice(&data[..n]);
                self.profile_slot = Some(k);
                self.blank &= !(1 << k);
            }
        }
        Ok(())
    }
}
