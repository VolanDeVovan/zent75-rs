//! The config banks on raw NOR flash, in the stock 0.0.18 layout (docs/re-peripherals.md §6):
//!
//! | Slot        | Flash                      | Stock writes | We keep            |
//! |-------------|----------------------------|--------------|--------------------|
//! | bank 1      | `0x80070000` (1 sector)    | `0x790` B    | whole 4 KB sector  |
//! | profile k   | `0x80080000 + k·0x2000`    | `0x1E38` B   | whole 8 KB slot    |
//!
//! **Write gate.** [`BankStore`] reaches the device's erase/program only through two private
//! functions that first check alignment and [`check_write_range`]; anything else is refused and
//! counted ([`Stats::refused`]), never forwarded.
//!
//! **Staging.** Reads come straight from the device. A `store` that changes something is staged
//! in RAM (cheap — it may run while the scan interrupt is masked) and written by
//! [`BankStore::commit`], which the platform calls from a low-priority context: debounced, and
//! forced before a reset. RAM: one bank-1 image and one profile image; an all-`FF` store (wipe)
//! needs no image at all.

use super::{check_write_range, Slot, Storage, StorageError};
use crate::config::{BANK1_SIZE, BANK2_SIZE};
use crate::layout::PROFILE_COUNT;

pub const SECTOR_SIZE: u32 = 4096;
pub const PAGE_SIZE: u32 = 256;
/// Bank 1.
pub const GLOBAL_ADDR: u32 = 0x8007_0000;
/// Profile `k` lives at `PROFILE_ADDR + k · PROFILE_STRIDE`.
pub const PROFILE_ADDR: u32 = 0x8008_0000;
pub const PROFILE_STRIDE: u32 = 0x2000;

// The ROM programs whole words; a slot never outgrows its sectors.
const _: () = assert!(BANK1_SIZE % 4 == 0 && BANK2_SIZE % 4 == 0);
const _: () = assert!(BANK1_SIZE <= SECTOR_SIZE as usize && BANK2_SIZE <= PROFILE_STRIDE as usize);
const _: () = assert!(check_write_range(GLOBAL_ADDR, SECTOR_SIZE));
const _: () = assert!(check_write_range(PROFILE_ADDR, PROFILE_STRIDE * PROFILE_COUNT as u32));

/// The device failed an erase or program (the platform knows and logs the details).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeviceError;

/// Raw NOR flash as the platform provides it. Addresses are absolute (`0x8xxx_xxxx`).
pub trait Nor {
    /// Copy `out.len()` bytes starting at `addr`.
    fn read(&mut self, addr: u32, out: &mut [u8]);
    /// Erase the [`SECTOR_SIZE`] sector starting at `addr` (sector-aligned).
    fn erase_sector(&mut self, addr: u32) -> Result<(), DeviceError>;
    /// Program `data` (1..=[`PAGE_SIZE`] bytes, a multiple of 4, inside one page) at `addr`.
    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), DeviceError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WriteError {
    /// Outside the config area or misaligned — refused before reaching the device.
    Refused,
    Device,
    /// Read-back after erase/program differs.
    Verify,
}

impl From<DeviceError> for WriteError {
    fn from(_: DeviceError) -> Self {
        WriteError::Device
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Stats {
    /// Slots written successfully.
    pub commits: u32,
    /// Slots that could not be written (after one retry). The keyboard's RAM copy stays
    /// authoritative; the slot is not retried until it is stored again.
    pub errors: u32,
    /// Erase/program requests stopped by the write gate. Must stay 0.
    pub refused: u32,
}

/// Where the current content of a slot lives.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    Flash,
    Staged,
    /// Pending erase: reads as all `FF`.
    Blank,
}

pub struct BankStore<N> {
    nor: N,
    global: [u8; BANK1_SIZE],
    global_dirty: bool,
    profile: [u8; BANK2_SIZE],
    /// Profile whose image is staged in `profile` (dirty).
    staged_profile: Option<u8>,
    /// Profiles to erase (bit k).
    blank: u8,
    /// A `store` staged something since the last `commit` call.
    touched: bool,
    last_change_ms: u64,
    stats: Stats,
}

fn slot_addr(slot: Slot) -> Option<(u32, usize)> {
    match slot {
        Slot::Global => Some((GLOBAL_ADDR, BANK1_SIZE)),
        Slot::Profile(k) if (k as usize) < PROFILE_COUNT => Some((PROFILE_ADDR + k as u32 * PROFILE_STRIDE, BANK2_SIZE)),
        Slot::Profile(_) => None,
    }
}

impl<N: Nor> BankStore<N> {
    pub const fn new(nor: N) -> Self {
        Self {
            nor,
            global: [0; BANK1_SIZE],
            global_dirty: false,
            profile: [0; BANK2_SIZE],
            staged_profile: None,
            blank: 0,
            touched: false,
            last_change_ms: 0,
            stats: Stats { commits: 0, errors: 0, refused: 0 },
        }
    }

    pub fn nor(&mut self) -> &mut N {
        &mut self.nor
    }

    pub fn stats(&self) -> Stats {
        self.stats
    }

    /// Something is staged and not yet on flash.
    pub fn dirty(&self) -> bool {
        self.global_dirty || self.staged_profile.is_some() || self.blank != 0
    }

    // ------------------------------------------------------------------ the write gate

    fn erase_sector(&mut self, addr: u32) -> Result<(), WriteError> {
        if !addr.is_multiple_of(SECTOR_SIZE) || !check_write_range(addr, SECTOR_SIZE) {
            self.stats.refused += 1;
            return Err(WriteError::Refused);
        }
        Ok(self.nor.erase_sector(addr)?)
    }

    fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), WriteError> {
        let len = data.len() as u32;
        let in_page = len > 0 && len.is_multiple_of(4) && addr % PAGE_SIZE + len <= PAGE_SIZE;
        if !in_page || !check_write_range(addr, len) {
            self.stats.refused += 1;
            return Err(WriteError::Refused);
        }
        Ok(self.nor.program(addr, data)?)
    }

    // ------------------------------------------------------------------ reading

    fn source(&self, slot: Slot) -> Source {
        match slot {
            Slot::Global if self.global_dirty => Source::Staged,
            Slot::Profile(k) if self.staged_profile == Some(k) => Source::Staged,
            Slot::Profile(k) if self.blank & (1 << k) != 0 => Source::Blank,
            _ => Source::Flash,
        }
    }

    /// Current content of `slot` from byte `off` (staged image, pending erase or flash).
    fn content(&mut self, slot: Slot, off: usize, out: &mut [u8]) {
        let Some((addr, _)) = slot_addr(slot) else { return };
        match self.source(slot) {
            Source::Staged if slot == Slot::Global => out.copy_from_slice(&self.global[off..off + out.len()]),
            Source::Staged => out.copy_from_slice(&self.profile[off..off + out.len()]),
            Source::Blank => out.fill(0xFF),
            Source::Flash => self.nor.read(addr + off as u32, out),
        }
    }

    /// `f(offset, chunk)` over the first `len` bytes of the current content of `slot`; stops
    /// early (returning false) when `f` does.
    fn scan_content(&mut self, slot: Slot, len: usize, mut f: impl FnMut(usize, &[u8]) -> bool) -> bool {
        let mut chunk = [0u8; 64];
        let mut off = 0;
        while off < len {
            let n = (len - off).min(chunk.len());
            self.content(slot, off, &mut chunk[..n]);
            if !f(off, &chunk[..n]) {
                return false;
            }
            off += n;
        }
        true
    }

    // ------------------------------------------------------------------ writing

    /// Write every dirty slot if `force`, or once the last change is `debounce_ms` old.
    /// Returns the number of slots written. An erase blocks for tens of milliseconds: call it
    /// from a low-priority context, never where `store` runs.
    pub fn commit(&mut self, now_ms: u64, debounce_ms: u64, force: bool) -> usize {
        if core::mem::take(&mut self.touched) {
            self.last_change_ms = now_ms;
        }
        if !self.dirty() || (!force && now_ms.saturating_sub(self.last_change_ms) < debounce_ms) {
            return 0;
        }
        let mut n = 0;
        if core::mem::take(&mut self.global_dirty) {
            n += self.finish(|s| s.write_image(GLOBAL_ADDR, Slot::Global));
        }
        if let Some(k) = self.staged_profile.take() {
            n += self.finish(|s| s.write_image(PROFILE_ADDR + k as u32 * PROFILE_STRIDE, Slot::Profile(k)));
        }
        for k in 0..PROFILE_COUNT as u32 {
            if self.blank & (1 << k) != 0 {
                self.blank &= !(1 << k);
                n += self.finish(|s| s.erase_region(PROFILE_ADDR + k * PROFILE_STRIDE, PROFILE_STRIDE));
            }
        }
        n
    }

    /// Run one slot write with a single retry and book the outcome.
    fn finish(&mut self, mut write: impl FnMut(&mut Self) -> Result<(), WriteError>) -> usize {
        if write(self).or_else(|_| write(self)).is_ok() {
            self.stats.commits += 1;
            1
        } else {
            self.stats.errors += 1;
            0
        }
    }

    /// Erase + program + verify the staged image of `slot` at `addr`, sector by sector. The
    /// rest of the slot's sectors stays erased, like stock.
    fn write_image(&mut self, addr: u32, slot: Slot) -> Result<(), WriteError> {
        let len = if slot == Slot::Global { BANK1_SIZE } else { BANK2_SIZE };
        let mut page = [0u8; PAGE_SIZE as usize];
        let mut check = [0u8; PAGE_SIZE as usize];
        let mut off = 0;
        while off < len {
            if (off as u32).is_multiple_of(SECTOR_SIZE) {
                self.erase_sector(addr + off as u32)?;
            }
            let n = (len - off).min(PAGE_SIZE as usize);
            let image: &[u8] = if slot == Slot::Global { &self.global } else { &self.profile };
            page[..n].copy_from_slice(&image[off..off + n]);
            // erased flash already reads FF: nothing to program for an all-FF page
            if page[..n].iter().any(|&b| b != 0xFF) {
                self.program(addr + off as u32, &page[..n])?;
            }
            self.nor.read(addr + off as u32, &mut check[..n]);
            if check[..n] != page[..n] {
                return Err(WriteError::Verify);
            }
            off += n;
        }
        Ok(())
    }

    /// Erase `len` bytes (whole sectors) and verify they read `FF`.
    fn erase_region(&mut self, addr: u32, len: u32) -> Result<(), WriteError> {
        let mut check = [0u8; 64];
        for sector in (addr..addr + len).step_by(SECTOR_SIZE as usize) {
            self.erase_sector(sector)?;
            for a in (sector..sector + SECTOR_SIZE).step_by(check.len()) {
                self.nor.read(a, &mut check);
                if check.iter().any(|&b| b != 0xFF) {
                    return Err(WriteError::Verify);
                }
            }
        }
        Ok(())
    }
}

impl<N: Nor> Storage for BankStore<N> {
    /// A slot that reads all `FF` (erased) or all `00` counts as empty; anything else is
    /// returned as-is, so banks the stock firmware wrote are used unchanged.
    fn load(&mut self, slot: Slot, buf: &mut [u8]) -> Result<(), StorageError> {
        let (_, len) = slot_addr(slot).ok_or(StorageError::Io)?;
        let n = buf.len().min(len);
        let (mut all_ff, mut all_00) = (true, true);
        self.scan_content(slot, n, |off, chunk| {
            all_ff &= chunk.iter().all(|&b| b == 0xFF);
            all_00 &= chunk.iter().all(|&b| b == 0x00);
            buf[off..off + chunk.len()].copy_from_slice(chunk);
            true
        });
        if all_ff || all_00 { Err(StorageError::Empty) } else { Ok(()) }
    }

    fn store(&mut self, slot: Slot, data: &[u8]) -> Result<(), StorageError> {
        let (addr, len) = slot_addr(slot).ok_or(StorageError::Io)?;
        let n = data.len().min(len);
        if self.scan_content(slot, n, |off, chunk| chunk == &data[off..off + chunk.len()]) {
            return Ok(()); // unchanged: no flash wear
        }
        self.touched = true;
        match slot {
            Slot::Global => {
                if !self.global_dirty {
                    // partial store: keep the rest of the current content
                    self.nor.read(addr + n as u32, &mut self.global[n..]);
                }
                self.global[..n].copy_from_slice(&data[..n]);
                self.global_dirty = true;
            }
            Slot::Profile(k) => {
                if n == len && data.iter().all(|&b| b == 0xFF) {
                    // erase only (wipe): no image needed
                    if self.staged_profile == Some(k) {
                        self.staged_profile = None;
                    }
                    self.blank |= 1 << k;
                    return Ok(());
                }
                if let Some(j) = self.staged_profile.filter(|&j| j != k) {
                    // Only one profile image is staged. Two different profiles inside one
                    // debounce window (a save, then a switch that repairs the new profile):
                    // write the older one now. Stock always saves with interrupts masked, so
                    // this ~100 ms pause is no regression.
                    self.staged_profile = None;
                    self.finish(|s| s.write_image(PROFILE_ADDR + j as u32 * PROFILE_STRIDE, Slot::Profile(j)));
                }
                if self.staged_profile != Some(k) {
                    if self.blank & (1 << k) != 0 {
                        self.profile[n..].fill(0xFF);
                    } else {
                        self.nor.read(addr + n as u32, &mut self.profile[n..]);
                    }
                }
                self.profile[..n].copy_from_slice(&data[..n]);
                self.staged_profile = Some(k);
                self.blank &= !(1 << k);
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const BASE: u32 = 0x8000_0000;

    /// 1 MB of NOR: erase sets a sector to FF, program can only clear bits. Records every
    /// device call; `fail_programs` makes the next n programs fail.
    struct FakeNor {
        mem: Vec<u8>,
        erases: Vec<u32>,
        programs: Vec<(u32, usize)>,
        fail_programs: usize,
    }

    impl FakeNor {
        fn new() -> Self {
            let mut mem = vec![0xFFu8; 0x10_0000];
            mem[..0x7_0000].fill(0xA5); // boot region + app: must never change
            Self { mem, erases: vec![], programs: vec![], fail_programs: 0 }
        }
        fn slice(&self, addr: u32, len: usize) -> &[u8] {
            &self.mem[(addr - BASE) as usize..][..len]
        }
    }

    impl Nor for FakeNor {
        fn read(&mut self, addr: u32, out: &mut [u8]) {
            out.copy_from_slice(self.slice(addr, out.len()));
        }
        fn erase_sector(&mut self, addr: u32) -> Result<(), DeviceError> {
            assert!(addr >= GLOBAL_ADDR && addr.is_multiple_of(SECTOR_SIZE), "erase {addr:#x} reached the device");
            self.erases.push(addr);
            self.mem[(addr - BASE) as usize..][..SECTOR_SIZE as usize].fill(0xFF);
            Ok(())
        }
        fn program(&mut self, addr: u32, data: &[u8]) -> Result<(), DeviceError> {
            assert!(addr >= GLOBAL_ADDR, "program {addr:#x} reached the device");
            assert!(addr % PAGE_SIZE + data.len() as u32 <= PAGE_SIZE && data.len().is_multiple_of(4));
            if self.fail_programs > 0 {
                self.fail_programs -= 1;
                return Err(DeviceError);
            }
            self.programs.push((addr, data.len()));
            for (m, d) in self.mem[(addr - BASE) as usize..].iter_mut().zip(data) {
                *m &= d;
            }
            Ok(())
        }
    }

    fn pattern(len: usize, seed: u8) -> Vec<u8> {
        (0..len).map(|i| (i as u8).wrapping_mul(31).wrapping_add(seed) | 1).collect()
    }

    fn untouched_below_config(s: &mut BankStore<FakeNor>) -> bool {
        s.nor().mem[..0x7_0000].iter().all(|&b| b == 0xA5)
    }

    #[test]
    fn blank_and_zero_slots_are_empty() {
        let mut s = BankStore::new(FakeNor::new());
        let mut buf = [0u8; BANK1_SIZE];
        assert_eq!(s.load(Slot::Global, &mut buf), Err(StorageError::Empty));
        s.nor().mem[0x8_0000..0x8_2000].fill(0);
        let mut p = [0u8; BANK2_SIZE];
        assert_eq!(s.load(Slot::Profile(0), &mut p), Err(StorageError::Empty));
        assert_eq!(s.load(Slot::Profile(4), &mut p), Err(StorageError::Io));
        // anything else is taken as-is (a bank the stock firmware wrote)
        s.nor().mem[0x7_0000 + 5] = 0x12;
        assert_eq!(s.load(Slot::Global, &mut buf), Ok(()));
        assert_eq!((buf[4], buf[5], buf[6]), (0xFF, 0x12, 0xFF));
    }

    #[test]
    fn store_is_staged_then_committed_after_the_debounce() {
        let mut s = BankStore::new(FakeNor::new());
        let g = pattern(BANK1_SIZE, 3);
        s.store(Slot::Global, &g).unwrap();
        assert!(s.dirty() && s.nor().erases.is_empty(), "store alone never touches the device");
        // the staged image is what a load sees
        let mut buf = [0u8; BANK1_SIZE];
        s.load(Slot::Global, &mut buf).unwrap();
        assert_eq!(&buf[..], &g[..]);
        assert_eq!(s.commit(1000, 300, false), 0, "debounce starts at the first commit call");
        assert_eq!(s.commit(1299, 300, false), 0);
        assert_eq!(s.commit(1300, 300, false), 1);
        assert!(!s.dirty());
        assert_eq!(s.nor().erases, [GLOBAL_ADDR]);
        assert_eq!(s.nor().slice(GLOBAL_ADDR, BANK1_SIZE), &g[..]);
        assert!(s.nor().slice(GLOBAL_ADDR + BANK1_SIZE as u32, 0x1000 - BANK1_SIZE).iter().all(|&b| b == 0xFF));
        assert_eq!(s.stats(), Stats { commits: 1, errors: 0, refused: 0 });
        // storing the same bytes again costs nothing
        s.store(Slot::Global, &g).unwrap();
        assert!(!s.dirty());
        assert!(untouched_below_config(&mut s));
    }

    #[test]
    fn profile_spans_two_sectors_and_skips_blank_pages() {
        let mut s = BankStore::new(FakeNor::new());
        let mut p = pattern(BANK2_SIZE, 9);
        p[0x300..0x400].fill(0xFF); // one all-FF page
        s.store(Slot::Profile(2), &p).unwrap();
        assert_eq!(s.commit(0, 0, true), 1);
        let addr = PROFILE_ADDR + 2 * PROFILE_STRIDE;
        assert_eq!(s.nor().erases, [addr, addr + SECTOR_SIZE]);
        assert_eq!(s.nor().slice(addr, BANK2_SIZE), &p[..]);
        assert!(!s.nor().programs.iter().any(|&(a, _)| a == addr + 0x300));
        assert_eq!(s.nor().programs.last(), Some(&(addr + 0x1E00, 0x38)));
        // neighbours untouched
        assert!(s.nor().slice(addr - PROFILE_STRIDE, 0x2000).iter().all(|&b| b == 0xFF));
        assert!(s.nor().slice(addr + PROFILE_STRIDE, 0x2000).iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn partial_store_keeps_the_rest_of_the_slot() {
        let mut s = BankStore::new(FakeNor::new());
        let g = pattern(BANK1_SIZE, 1);
        s.store(Slot::Global, &g).unwrap();
        s.commit(0, 0, true);
        s.store(Slot::Global, &[0x42; 16]).unwrap();
        s.commit(0, 0, true);
        assert_eq!(s.nor().slice(GLOBAL_ADDR, 16), &[0x42; 16]);
        assert_eq!(s.nor().slice(GLOBAL_ADDR + 16, BANK1_SIZE - 16), &g[16..]);
    }

    #[test]
    fn wipe_erases_without_an_image_and_reads_blank_meanwhile() {
        let mut s = BankStore::new(FakeNor::new());
        let p = pattern(BANK2_SIZE, 5);
        for k in 0..4 {
            s.store(Slot::Profile(k), &p).unwrap();
            s.commit(0, 0, true);
        }
        let n = s.nor().erases.len();
        for k in 0..4 {
            s.store(Slot::Profile(k), &[0xFF; BANK2_SIZE]).unwrap();
        }
        assert_eq!(s.nor().erases.len(), n, "nothing written before the commit");
        let mut buf = [0u8; BANK2_SIZE];
        assert_eq!(s.load(Slot::Profile(1), &mut buf), Err(StorageError::Empty));
        assert_eq!(s.commit(0, 0, true), 4);
        assert!(s.nor().slice(PROFILE_ADDR, 4 * 0x2000).iter().all(|&b| b == 0xFF));
        // a store after a pending erase starts from a blank slot
        s.store(Slot::Profile(1), &[0xFF; BANK2_SIZE]).unwrap();
        s.store(Slot::Profile(1), &[7; 8]).unwrap();
        s.commit(0, 0, true);
        assert_eq!(s.nor().slice(PROFILE_ADDR + 0x2000, 8), &[7; 8]);
        assert!(s.nor().slice(PROFILE_ADDR + 0x2008, BANK2_SIZE - 8).iter().all(|&b| b == 0xFF));
    }

    #[test]
    fn a_second_profile_flushes_the_first() {
        let mut s = BankStore::new(FakeNor::new());
        let (a, b) = (pattern(BANK2_SIZE, 1), pattern(BANK2_SIZE, 2));
        s.store(Slot::Profile(0), &a).unwrap();
        s.store(Slot::Profile(1), &b).unwrap();
        assert_eq!(s.nor().slice(PROFILE_ADDR, BANK2_SIZE), &a[..], "profile 0 written early");
        assert_eq!(s.commit(0, 0, true), 1);
        assert_eq!(s.nor().slice(PROFILE_ADDR + 0x2000, BANK2_SIZE), &b[..]);
        assert_eq!(s.stats().commits, 2);
    }

    #[test]
    fn device_failure_is_retried_once_then_counted() {
        let mut s = BankStore::new(FakeNor::new());
        let g = pattern(BANK1_SIZE, 3);
        s.nor().fail_programs = 1;
        s.store(Slot::Global, &g).unwrap();
        assert_eq!(s.commit(0, 0, true), 1, "second attempt succeeds");
        assert_eq!(s.nor().slice(GLOBAL_ADDR, BANK1_SIZE), &g[..]);
        s.nor().fail_programs = 2;
        s.store(Slot::Global, &pattern(BANK1_SIZE, 4)).unwrap();
        assert_eq!(s.commit(0, 0, true), 0);
        assert_eq!(s.stats(), Stats { commits: 1, errors: 1, refused: 0 });
        assert!(!s.dirty(), "not retried forever");
    }

    #[test]
    fn the_gate_refuses_everything_outside_the_config_area() {
        let mut s = BankStore::new(FakeNor::new());
        // the fake device asserts if any of these reached it
        for addr in [0x8000_0000, 0x8002_0000, 0x8003_F000, 0x8004_0000, 0x8006_F000, 0x8010_0000, 0x7FFF_F000] {
            assert_eq!(s.erase_sector(addr), Err(WriteError::Refused), "{addr:#x}");
            assert_eq!(s.program(addr, &[0; 16]), Err(WriteError::Refused), "{addr:#x}");
        }
        assert_eq!(s.erase_sector(GLOBAL_ADDR + 4), Err(WriteError::Refused), "unaligned erase");
        assert_eq!(s.program(GLOBAL_ADDR + 0xF0, &[0; 32]), Err(WriteError::Refused), "crosses a page");
        assert_eq!(s.program(GLOBAL_ADDR, &[0; 3]), Err(WriteError::Refused), "not whole words");
        assert_eq!(s.program(GLOBAL_ADDR, &[]), Err(WriteError::Refused));
        assert_eq!(s.program(0x800F_FF00, &[0; 256]), Ok(()), "last page of the flash");
        assert_eq!(s.erase_sector(0x800F_F000), Ok(()));
        assert_eq!(s.stats().refused, 18);
        assert!(s.nor().erases == [0x800F_F000] && untouched_below_config(&mut s));
    }
}
