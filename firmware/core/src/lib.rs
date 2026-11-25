//! Hardware-independent core of the ZENT75-HE firmware.
//!
//! Everything here is `no_std`, allocation-free and deterministic so it runs unchanged on the
//! HPM5361 (`firmware/app`), in the host simulator (`firmware/sim`) and in unit tests.
#![cfg_attr(not(any(test, feature = "std")), no_std)]

pub mod config;
pub mod defaults;
pub mod device;
pub mod engine;
pub mod hid;
pub mod keycode;
pub mod keys;
pub mod lighting;
pub mod storage;
pub mod crc;
pub mod layout;
pub mod protocol;
