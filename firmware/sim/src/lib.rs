//! Host-side simulator for the ZENT75-HE firmware core (`firmware/core`).
//!
//! * [`sensor`]: physical travel → ADC samples
//! * [`sim`]: the board (Keyboard + storage + virtual HID) and the real-time loop
//! * [`uhid`]: `/dev/uhid` virtual HID devices (product id 0x5A11, never the real 0x357E)
//! * [`control`], [`command`]: control socket / script line protocol
//! * [`store`], [`backup`]: directory-backed config banks, seeding from `tools/dump_all.py` dumps

pub mod backup;
pub mod command;
pub mod control;
pub mod hid;
pub mod json;
pub mod keynames;
pub mod sensor;
pub mod sim;
pub mod store;
pub mod uhid;
