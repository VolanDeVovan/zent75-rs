//! Hardware-independent core of the ZENT75-HE firmware.
//!
//! Everything here is `no_std`, allocation-free, `unsafe`-free and deterministic, so it runs
//! unchanged on the HPM5361 (`firmware/app`), in the host simulator (`firmware/sim`) and in
//! tests. A platform owns the hardware and drives one [`device::Keyboard`]:
//!
//! ```text
//!            samples ──► engine ──► keys ──► hid reports ──► USB / uhid
//!                           │         │
//!  vendor frames ◄─► device::vendor   └──► lighting ──► LED frame
//!                           │
//!                 config (bank images) ◄─► storage ◄─► NOR / files
//! ```
//!
//! * [`config`], [`defaults`], [`keycode`], [`layout`] — the data model: the two config banks
//!   in the stock byte layout with typed accessors, factory images, keymap codes, key / slot /
//!   LED tables.
//! * [`engine`] — ADC samples → travel; [`keys`] — travel → HID reports ([`hid`]);
//!   [`lighting`] — the RGB engine. All three are ports of the stock 0.0.18 behaviour,
//!   diff-tested against it; deliberate deviations are switches in [`engine::compat`].
//! * [`protocol`], [`crc`] — configurator frames; dispatched in [`device`].
//! * [`storage`] — persistence interface, the flash write-range guard and the NOR bank store.
#![cfg_attr(not(any(test, feature = "std")), no_std)]

pub mod config;
pub mod crc;
pub mod defaults;
pub mod device;
pub mod engine;
pub mod hid;
pub mod keycode;
pub mod keys;
pub mod layout;
pub mod lighting;
pub mod protocol;
pub mod storage;
