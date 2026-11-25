//! Persistent storage of the config banks.
//!
//! [`Keyboard`](crate::device::Keyboard) decides *what* to persist and talks to a [`Storage`]:
//! two kinds of blobs, bank 1 and one bank-2 image per profile. Implementations:
//!
//! * [`nor::BankStore`] — the board: the stock NOR layout on a raw [`nor::Nor`] device, with
//!   RAM staging, debounced commits and read-back verification;
//! * `zent-sim`'s directory of files;
//! * [`mem::MemStorage`] for tests.
//!
//! [`check_write_range`] is the last line of defence against touching the boot region
//! (`0x8000_0000..0x8004_0000`, docs/boot-recovery.md): [`nor::BankStore`] passes every erase
//! and program through it, and a flash driver must check it again itself.

pub mod nor;

/// Where a blob lives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Slot {
    /// Board-global bank 1.
    Global,
    /// Bank 2 copy of profile `n` (0..4).
    Profile(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StorageError {
    /// Nothing stored yet / blank flash / bad checksum.
    Empty,
    Io,
}

pub trait Storage {
    /// Fill `buf` with the stored blob. `Err(Empty)` if there is none.
    fn load(&mut self, slot: Slot, buf: &mut [u8]) -> Result<(), StorageError>;
    /// Replace the stored blob.
    fn store(&mut self, slot: Slot, data: &[u8]) -> Result<(), StorageError>;
}

/// First flash address our firmware may ever erase or program.
pub const FLASH_WRITABLE_START: u32 = 0x8007_0000;
/// End (exclusive) of the 1 MB NOR.
pub const FLASH_WRITABLE_END: u32 = 0x8010_0000;

/// True if `[addr, addr+len)` lies entirely inside the config area. Flash drivers must refuse
/// anything else; there is no override.
pub const fn check_write_range(addr: u32, len: u32) -> bool {
    match addr.checked_add(len) {
        Some(end) => addr >= FLASH_WRITABLE_START && end <= FLASH_WRITABLE_END && len > 0,
        None => false,
    }
}

/// In-memory storage for tests and the simulator.
#[cfg(any(test, feature = "std"))]
pub mod mem {
    extern crate std;
    use super::*;
    use std::collections::BTreeMap;
    use std::vec::Vec;

    #[derive(Default, Clone)]
    pub struct MemStorage {
        pub blobs: BTreeMap<(u8, u8), Vec<u8>>,
        pub stores: usize,
    }

    fn key(s: Slot) -> (u8, u8) {
        match s {
            Slot::Global => (0, 0),
            Slot::Profile(n) => (1, n),
        }
    }

    impl Storage for MemStorage {
        fn load(&mut self, slot: Slot, buf: &mut [u8]) -> Result<(), StorageError> {
            let b = self.blobs.get(&key(slot)).ok_or(StorageError::Empty)?;
            let n = b.len().min(buf.len());
            buf[..n].copy_from_slice(&b[..n]);
            Ok(())
        }
        fn store(&mut self, slot: Slot, data: &[u8]) -> Result<(), StorageError> {
            self.stores += 1;
            self.blobs.insert(key(slot), data.to_vec());
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn guard() {
        assert!(!check_write_range(0x8003_FFFF, 1));
        assert!(!check_write_range(0x8004_0000, 0x1000));
        assert!(!check_write_range(0x8006_F000, 0x2000));
        assert!(check_write_range(0x8007_0000, 0x1000));
        assert!(check_write_range(0x800F_F000, 0x1000));
        assert!(!check_write_range(0x800F_F000, 0x1001));
        assert!(!check_write_range(0xFFFF_F000, 0x2000));
        assert!(!check_write_range(0x8007_0000, 0));
    }
}
