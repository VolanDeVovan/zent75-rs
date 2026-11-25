//! The configurator-protocol side of [`Keyboard`]: the dispatcher for host frames and the
//! unsolicited device frames.
//!
//! The dispatcher follows stock 0.0.18 `FUN_0000B812` (docs/re-config-rgb.md §3) with these
//! deliberate differences (**deviations**): writes never go past our shadows (the excess is
//! dropped; stock overwrote live RAM), reads outside the shadows return zeros (stock leaked
//! stack), `BE 01` is answered like `BE 04` (stock hangs), and the firmware-owned bank-1 words
//! `soft_version` / `cur_param_num` cannot be overwritten by `BE 03`.

use super::{Keyboard, SystemAction, HEARTBEAT_SCANS, STREAM_EVERY};
use crate::config::{self, b1, b2, Bank, BANK1_SIZE, BANK2_SIZE};
use crate::protocol::{cmd, normalize, parse, Reply, Request, MAX_RW, REPORT_SIZE, SOF};
use crate::storage::Storage;

impl Keyboard {
    /// Handle one vendor output report. Returns the length of a reply written to `out`.
    pub fn vendor_in(&mut self, report: &[u8], st: &mut impl Storage, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        let report = normalize(report);
        if report.first() == Some(&SOF) {
            // every frame that reaches the dispatcher keeps the LEDs awake (stock)
            self.lighting.note_activity();
        }
        match parse(report).ok()? {
            Request::Heartbeat => self.heartbeat_left = HEARTBEAT_SCANS,
            Request::Save => {
                // bank-1 save (stock `FUN_00015F5A`): store, re-impose the fixed Fn shortcuts,
                // then the profile
                self.save_global(st);
                self.profile.apply_fixed_fn(self.active_profile());
                self.save_profile(st);
            }
            Request::WriteRam { addr, data } => {
                let (bank, off) = config::locate_write(addr, data.len())?;
                match bank {
                    Bank::Global => self.write_global(off, data, st),
                    Bank::Profile => self.write_profile(off, data),
                    // live preview: RAM only, no ack (stock)
                    Bank::Preview => {
                        self.lighting.preview_write(off, data);
                        return None;
                    }
                }
                return Some(Reply::new(out).simple(cmd::WRITE_RAM));
            }
            Request::Read { cmd, addr, n } => {
                let src: &[u8] = match config::locate_read(addr, n as usize) {
                    Some((Bank::Global, off)) => self.global.0.get(off..).unwrap_or(&[]),
                    Some((Bank::Profile, off)) => self.profile.0.get(off..).unwrap_or(&[]),
                    _ => &[],
                };
                return Some(Reply::new(out).data(cmd, addr, n, |d| {
                    let have = src.len().min(d.len());
                    d[..have].copy_from_slice(&src[..have]);
                    d[have..].fill(0);
                }));
            }
            Request::Serial => return Some(Reply::new(out).serial(&self.serial)),
            Request::SwitchProfile(p) => self.request_switch(p, true),
            Request::Diag(v) => {
                if v == 2 || v == 3 {
                    self.engine.set_diag(v == 3);
                }
            }
            Request::ResetConfig => self.reset_profile(st),
            Request::Bootloader => self.pending.action = Some(SystemAction::EnterBootloader),
            Request::Calibration(on) => self.engine.set_calibration(on),
            Request::Reboot => self.pending.action = Some(SystemAction::Reboot),
        }
        None
    }

    /// `BE 03` into bank 1: copied into the shadow, then saved and re-derived — unless any key
    /// now has `|bot − top| < 3000`, in which case the write is discarded (stock re-reads NOR;
    /// we restore the pre-write shadow, which is the same bytes). The ack follows the save.
    fn write_global(&mut self, off: usize, data: &[u8], st: &mut impl Storage) {
        const OWNED: core::ops::Range<usize> = b1::SOFT_VERSION..b1::CUR_PARAM_NUM + 4;
        let end = (off + data.len()).min(BANK1_SIZE);
        if off >= end {
            return; // entirely past the shadow (stock: preview area / live RAM)
        }
        let n = end - off;
        // keep the overwritten bytes for the discard case, not a copy of the bank
        let mut old = [0u8; MAX_RW];
        old[..n].copy_from_slice(&self.global.0[off..end]);
        let mut owned = [0u8; OWNED.end - OWNED.start];
        owned.copy_from_slice(&self.global.0[OWNED]);
        self.global.0[off..end].copy_from_slice(&data[..n]);
        // firmware-owned words (deviation: stock lets the host overwrite them)
        self.global.0[OWNED].copy_from_slice(&owned);
        if self.global.calibration_implausible() {
            self.global.0[off..end].copy_from_slice(&old[..n]);
            self.global.0[OWNED].copy_from_slice(&owned);
            return;
        }
        self.save_global(st);
        self.profile.apply_fixed_fn(self.active_profile());
        self.apply_config();
    }

    /// `BE 03` into the profile: RAM only (saved by `BE 02`). Writes inside the SOCD / static
    /// colour window only reload the lighting, everything else re-derives keys + lighting.
    fn write_profile(&mut self, off: usize, data: &[u8]) {
        let end_req = off + data.len();
        let end = end_req.min(BANK2_SIZE);
        if off < end {
            self.profile.0[off..end].copy_from_slice(&data[..end - off]);
        }
        if off >= b2::LIGHT_WINDOW_START && end_req <= b2::LIGHT_WINDOW_END {
            self.lighting.configure(&mut self.profile);
        } else {
            self.apply_config();
        }
    }

    /// Unsolicited frames: profile notice first, then calibration frames, then the live stream
    /// while heartbeats arrive.
    pub fn vendor_poll(&mut self, out: &mut [u8; REPORT_SIZE]) -> Option<usize> {
        if let Some(p) = self.pending.profile_notice.take() {
            return Some(Reply::new(out).with_arg(cmd::SWITCH_PROFILE, p));
        }
        if let Some(n) = self.engine.take_calibration_frame(out) {
            return Some(n); // calibration mode: no heartbeat needed (stock)
        }
        if self.heartbeat_left == 0 || self.scans.wrapping_sub(self.stream_mark) < STREAM_EVERY {
            return None;
        }
        self.stream_mark = self.scans;
        Some(self.engine.stream_frame(out))
    }
}
