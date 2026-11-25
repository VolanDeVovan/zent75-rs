//! Vendor configurator protocol framing (docs/protocol.md).
//!
//! ```text
//! BE <cmd> <len:LE16> <payload…> [<crc:LE16>] ED      zero-padded to 1024 bytes
//! ```
//! `len` counts the whole frame. Requests carry `addr` LE32 at bytes 4..8 and `n` **BE16** at
//! bytes 8..10. CRC-16/MODBUS covers bytes `0 .. len-3`.

use crate::crc::crc16_modbus;

pub const REPORT_SIZE: usize = 1024;
pub const SOF: u8 = 0xBE;
pub const EOF: u8 = 0xED;
/// Header (4) + addr (4) + n (2) + crc (2) + ED (1).
pub const DATA_OVERHEAD: usize = 13;
/// Largest `n` the stock firmware accepts for `BE 03`/`BE 04`.
pub const MAX_RW: usize = 512;

pub mod cmd {
    pub const HEARTBEAT: u8 = 0x00;
    pub const READ_FLASH: u8 = 0x01;
    pub const SAVE: u8 = 0x02;
    pub const WRITE_RAM: u8 = 0x03;
    pub const READ_RAM: u8 = 0x04;
    pub const SERIAL: u8 = 0x1D;
    pub const SWITCH_PROFILE: u8 = 0xC0;
    pub const DIAG: u8 = 0xDA;
    pub const RESET_CONFIG: u8 = 0xEE;
    pub const BOOTLOADER: u8 = 0xFC;
    pub const STREAM: u8 = 0xFD;
    pub const CALIBRATION: u8 = 0xFE;
    pub const REBOOT: u8 = 0xFF;
}

/// A decoded host → device request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Request<'a> {
    Heartbeat,
    /// `BE 01`: the stock firmware hangs on it. We answer like a RAM read (safe), see device.rs.
    ReadFlash { addr: u32, n: u16 },
    Save,
    WriteRam { addr: u32, data: &'a [u8] },
    ReadRam { addr: u32, n: u16 },
    Serial,
    SwitchProfile(u8),
    Diag(u8),
    ResetConfig,
    Bootloader,
    Calibration(bool),
    Reboot,
}

/// Why a frame was dropped (the stock firmware drops all of these silently).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameError {
    Short,
    BadSof,
    BadLen,
    BadEof,
    BadCrc,
    TooLong,
    Unknown(u8),
}

/// Strip a leading report id 0 that hidraw may add (1025-byte reports).
pub fn normalize(report: &[u8]) -> &[u8] {
    if report.len() == REPORT_SIZE + 1 && report[0] == 0 && report[1] == SOF {
        &report[1..]
    } else {
        report
    }
}

/// Parse a 1024-byte output report into a request.
pub fn parse(report: &[u8]) -> Result<Request<'_>, FrameError> {
    let r = normalize(report);
    if r.len() < 5 {
        return Err(FrameError::Short);
    }
    if r[0] != SOF {
        return Err(FrameError::BadSof);
    }
    let len = u16::from_le_bytes([r[2], r[3]]) as usize;
    if !(5..=REPORT_SIZE).contains(&len) || len > r.len() {
        return Err(FrameError::BadLen);
    }
    if r[len - 1] != EOF {
        return Err(FrameError::BadEof);
    }
    let c = r[1];
    let arg = |want: usize| -> Result<u8, FrameError> {
        if len == want { Ok(r[4]) } else { Err(FrameError::BadLen) }
    };
    let addr_n = || -> Result<(u32, u16), FrameError> {
        if len < 11 {
            return Err(FrameError::BadLen);
        }
        Ok((
            u32::from_le_bytes([r[4], r[5], r[6], r[7]]),
            u16::from_be_bytes([r[8], r[9]]),
        ))
    };
    Ok(match c {
        cmd::HEARTBEAT => Request::Heartbeat,
        cmd::SAVE => Request::Save,
        cmd::SERIAL => Request::Serial,
        cmd::RESET_CONFIG => Request::ResetConfig,
        cmd::BOOTLOADER => Request::Bootloader,
        cmd::REBOOT => Request::Reboot,
        cmd::SWITCH_PROFILE => Request::SwitchProfile(arg(6)?),
        cmd::DIAG => Request::Diag(arg(6)?),
        cmd::CALIBRATION => Request::Calibration(arg(6)? != 0),
        cmd::READ_RAM | cmd::READ_FLASH => {
            let (addr, n) = addr_n()?;
            if n as usize > MAX_RW || n == 0 {
                return Err(FrameError::TooLong);
            }
            if c == cmd::READ_RAM { Request::ReadRam { addr, n } } else { Request::ReadFlash { addr, n } }
        }
        cmd::WRITE_RAM => {
            let (addr, n) = addr_n()?;
            let n = n as usize;
            if n > MAX_RW || n == 0 || len != DATA_OVERHEAD + n {
                return Err(FrameError::BadLen);
            }
            let crc = u16::from_le_bytes([r[len - 3], r[len - 2]]);
            if crc16_modbus(&r[..len - 3]) != crc {
                return Err(FrameError::BadCrc);
            }
            Request::WriteRam { addr, data: &r[10..10 + n] }
        }
        other => return Err(FrameError::Unknown(other)),
    })
}

/// Builds device → host frames into a 1024-byte report buffer.
pub struct Reply<'a> {
    buf: &'a mut [u8; REPORT_SIZE],
}

impl<'a> Reply<'a> {
    pub fn new(buf: &'a mut [u8; REPORT_SIZE]) -> Self {
        buf.fill(0);
        Self { buf }
    }

    /// `BE <cmd> 05 00 ED` (write ack).
    pub fn simple(self, c: u8) -> usize {
        self.buf[..5].copy_from_slice(&[SOF, c, 5, 0, EOF]);
        5
    }

    /// `BE <cmd> 06 00 <arg> ED` (profile notice).
    pub fn with_arg(self, c: u8, arg: u8) -> usize {
        self.buf[..6].copy_from_slice(&[SOF, c, 6, 0, arg, EOF]);
        6
    }

    /// `BE 1D 15 00 <16 bytes> ED` (no CRC).
    pub fn serial(self, uid: &[u8; 16]) -> usize {
        self.buf[..4].copy_from_slice(&[SOF, cmd::SERIAL, 21, 0]);
        self.buf[4..20].copy_from_slice(uid);
        self.buf[20] = EOF;
        21
    }

    /// Read reply: `BE <cmd> <13+n> <addr LE32> <n BE16> <data> <crc> ED`. `fill` writes the
    /// `n` data bytes.
    pub fn data(self, c: u8, addr: u32, n: u16, fill: impl FnOnce(&mut [u8])) -> usize {
        let n_us = n as usize;
        let len = DATA_OVERHEAD + n_us;
        let b = &mut *self.buf;
        b[0] = SOF;
        b[1] = c;
        b[2..4].copy_from_slice(&(len as u16).to_le_bytes());
        b[4..8].copy_from_slice(&addr.to_le_bytes());
        b[8..10].copy_from_slice(&n.to_be_bytes());
        fill(&mut b[10..10 + n_us]);
        finish_crc(b, len);
        len
    }

    /// Stream frame `BE FD <len> <words…> <crc> ED`. `words` are written LE.
    pub fn stream(self, words: impl Iterator<Item = u16>, count: usize) -> usize {
        let len = 4 + 2 * count + 3;
        let b = &mut *self.buf;
        b[0] = SOF;
        b[1] = cmd::STREAM;
        b[2..4].copy_from_slice(&(len as u16).to_le_bytes());
        for (i, w) in words.take(count).enumerate() {
            b[4 + 2 * i..6 + 2 * i].copy_from_slice(&w.to_le_bytes());
        }
        finish_crc(b, len);
        len
    }
}

fn finish_crc(b: &mut [u8], len: usize) {
    let crc = crc16_modbus(&b[..len - 3]);
    b[len - 3..len - 1].copy_from_slice(&crc.to_le_bytes());
    b[len - 1] = EOF;
}

/// Host-side encoders (used by tests and the simulator's self-test).
pub mod host {
    use super::*;

    pub fn simple(c: u8) -> [u8; REPORT_SIZE] {
        let mut b = [0u8; REPORT_SIZE];
        b[..5].copy_from_slice(&[SOF, c, 5, 0, EOF]);
        b
    }

    pub fn with_arg(c: u8, arg: u8) -> [u8; REPORT_SIZE] {
        let mut b = [0u8; REPORT_SIZE];
        b[..6].copy_from_slice(&[SOF, c, 6, 0, arg, EOF]);
        b
    }

    pub fn read(c: u8, addr: u32, n: u16) -> [u8; REPORT_SIZE] {
        let mut b = [0u8; REPORT_SIZE];
        b[..4].copy_from_slice(&[SOF, c, 11, 0]);
        b[4..8].copy_from_slice(&addr.to_le_bytes());
        b[8..10].copy_from_slice(&n.to_be_bytes());
        b[10] = EOF;
        b
    }

    pub fn write(addr: u32, data: &[u8]) -> [u8; REPORT_SIZE] {
        let mut b = [0u8; REPORT_SIZE];
        let len = DATA_OVERHEAD + data.len();
        b[0] = SOF;
        b[1] = cmd::WRITE_RAM;
        b[2..4].copy_from_slice(&(len as u16).to_le_bytes());
        b[4..8].copy_from_slice(&addr.to_le_bytes());
        b[8..10].copy_from_slice(&(data.len() as u16).to_be_bytes());
        b[10..10 + data.len()].copy_from_slice(data);
        finish_crc(&mut b, len);
        b
    }

    /// Data of a read reply, checking CRC and echo.
    pub fn read_reply(r: &[u8], c: u8, addr: u32, n: u16) -> Option<&[u8]> {
        let len = u16::from_le_bytes([r[2], r[3]]) as usize;
        if r[0] != SOF || r[1] != c || len != DATA_OVERHEAD + n as usize || r[len - 1] != EOF {
            return None;
        }
        if u32::from_le_bytes([r[4], r[5], r[6], r[7]]) != addr || u16::from_be_bytes([r[8], r[9]]) != n {
            return None;
        }
        let crc = u16::from_le_bytes([r[len - 3], r[len - 2]]);
        (crc16_modbus(&r[..len - 3]) == crc).then(|| &r[10..len - 3])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_write() {
        let f = host::write(0x8008_0160, &[1, 2, 3]);
        assert_eq!(parse(&f), Ok(Request::WriteRam { addr: 0x8008_0160, data: &[1, 2, 3] }));
        let mut bad = f;
        bad[11] ^= 1;
        assert_eq!(parse(&bad), Err(FrameError::BadCrc));
    }

    #[test]
    fn read_and_reply() {
        let f = host::read(cmd::READ_RAM, 0x8007_0210, 4);
        assert_eq!(parse(&f), Ok(Request::ReadRam { addr: 0x8007_0210, n: 4 }));
        let mut out = [0u8; REPORT_SIZE];
        let len = Reply::new(&mut out).data(cmd::READ_RAM, 0x8007_0210, 4, |d| d.copy_from_slice(&[18, 0, 0, 0]));
        assert_eq!(len, 17);
        assert_eq!(host::read_reply(&out, cmd::READ_RAM, 0x8007_0210, 4), Some(&[18u8, 0, 0, 0][..]));
    }

    #[test]
    fn hidraw_prefix_and_garbage() {
        let mut r = [0u8; REPORT_SIZE + 1];
        r[1..].copy_from_slice(&host::simple(cmd::SAVE));
        assert_eq!(parse(&r), Ok(Request::Save));
        assert_eq!(parse(&[0u8; REPORT_SIZE]), Err(FrameError::BadSof));
        let mut x = host::simple(cmd::SAVE);
        x[4] = 0;
        assert_eq!(parse(&x), Err(FrameError::BadEof));
        assert_eq!(parse(&host::simple(0x42)), Err(FrameError::Unknown(0x42)));
    }

    #[test]
    fn stream_frame() {
        let mut out = [0u8; REPORT_SIZE];
        let len = Reply::new(&mut out).stream((0..505).map(|i| i as u16), 505);
        assert_eq!(len, 1017);
        assert_eq!(&out[..4], &[0xBE, 0xFD, 0xF9, 0x03]);
        assert_eq!(out[1016], EOF);
    }
}
