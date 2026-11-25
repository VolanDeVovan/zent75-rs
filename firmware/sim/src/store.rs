//! [`Storage`] over a directory: `global.bin` and `profile<N>.bin`, written atomically
//! (temp file + rename) so a killed simulator never leaves a torn blob.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use zent_core::storage::{Slot, Storage, StorageError};

pub struct DirStorage {
    dir: PathBuf,
    /// Number of successful stores (for logs and tests).
    pub stores: usize,
}

impl DirStorage {
    pub fn open(dir: impl Into<PathBuf>) -> io::Result<Self> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        Ok(Self { dir, stores: 0 })
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    pub fn path(&self, slot: Slot) -> PathBuf {
        self.dir.join(match slot {
            Slot::Global => "global.bin".to_string(),
            Slot::Profile(n) => format!("profile{n}.bin"),
        })
    }
}

impl Storage for DirStorage {
    fn load(&mut self, slot: Slot, buf: &mut [u8]) -> Result<(), StorageError> {
        match fs::read(self.path(slot)) {
            Ok(b) => {
                let n = b.len().min(buf.len());
                buf[..n].copy_from_slice(&b[..n]);
                buf[n..].fill(0);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => Err(StorageError::Empty),
            Err(_) => Err(StorageError::Io),
        }
    }

    fn store(&mut self, slot: Slot, data: &[u8]) -> Result<(), StorageError> {
        let path = self.path(slot);
        let tmp = path.with_extension("tmp");
        fs::write(&tmp, data).and_then(|_| fs::rename(&tmp, &path)).map_err(|_| StorageError::Io)?;
        self.stores += 1;
        Ok(())
    }
}
