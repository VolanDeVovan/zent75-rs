//! Minimal `/dev/uhid` client: raw `read`/`write` of `struct uhid_event` (linux/uhid.h).
//!
//! ```text
//! struct uhid_event { __u32 type; union { ... } u; } __attribute__((packed));
//! ```
//! The largest union member is `uhid_create2_req` (4372 bytes), so one event is 4376 bytes.
//! The kernel zero-extends short writes, and a read returns at most one event.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

pub const UHID_DESTROY: u32 = 1;
pub const UHID_START: u32 = 2;
pub const UHID_STOP: u32 = 3;
pub const UHID_OPEN: u32 = 4;
pub const UHID_CLOSE: u32 = 5;
pub const UHID_OUTPUT: u32 = 6;
pub const UHID_GET_REPORT: u32 = 9;
pub const UHID_GET_REPORT_REPLY: u32 = 10;
pub const UHID_CREATE2: u32 = 11;
pub const UHID_INPUT2: u32 = 12;
pub const UHID_SET_REPORT: u32 = 13;
pub const UHID_SET_REPORT_REPLY: u32 = 14;

/// `UHID_DATA_MAX` and `HID_MAX_DESCRIPTOR_SIZE`.
pub const DATA_MAX: usize = 4096;
/// `sizeof(struct uhid_create2_req)`: name 128 + phys 64 + uniq 64 + rd_size 2 + bus 2 +
/// vendor/product/version/country 4×4 + rd_data 4096.
pub const CREATE2_SIZE: usize = 128 + 64 + 64 + 2 + 2 + 16 + DATA_MAX;
/// `sizeof(struct uhid_event)`.
pub const EVENT_SIZE: usize = 4 + CREATE2_SIZE;
const _: () = assert!(EVENT_SIZE == 4376);

pub const BUS_USB: u16 = 0x03;
pub const UHID_FEATURE_REPORT: u8 = 0;
pub const UHID_OUTPUT_REPORT: u8 = 1;
pub const UHID_INPUT_REPORT: u8 = 2;

// Byte offsets inside the event (type at 0, union at 4).
mod off {
    pub const NAME: usize = 4;
    pub const PHYS: usize = 4 + 128;
    pub const UNIQ: usize = 4 + 192;
    pub const RD_SIZE: usize = 4 + 256;
    pub const BUS: usize = 4 + 258;
    pub const VENDOR: usize = 4 + 260;
    pub const PRODUCT: usize = 4 + 264;
    pub const VERSION: usize = 4 + 268;
    pub const COUNTRY: usize = 4 + 272;
    pub const RD_DATA: usize = 4 + 276;
    // uhid_input2_req
    pub const IN2_SIZE: usize = 4;
    pub const IN2_DATA: usize = 6;
    // uhid_output_req
    pub const OUT_DATA: usize = 4;
    pub const OUT_SIZE: usize = 4 + 4096;
    pub const OUT_RTYPE: usize = 4 + 4098;
    // uhid_get_report_req / uhid_set_report_req / replies
    pub const REQ_ID: usize = 4;
    pub const REQ_RNUM: usize = 8;
    pub const REQ_RTYPE: usize = 9;
    pub const SET_SIZE: usize = 10;
    pub const SET_DATA: usize = 12;
    pub const REPLY_ERR: usize = 8;
    pub const REPLY_SIZE: usize = 10;
    pub const REPLY_DATA: usize = 12;
    // uhid_start_req
    pub const START_FLAGS: usize = 4;
}

/// Identity of a virtual HID device.
#[derive(Debug, Clone)]
pub struct DeviceInfo {
    pub name: String,
    pub phys: String,
    pub uniq: String,
    pub bus: u16,
    pub vendor: u32,
    pub product: u32,
    pub version: u32,
    pub descriptor: Vec<u8>,
}

/// An event read from the kernel.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Start { flags: u64 },
    Stop,
    Open,
    Close,
    Output { data: Vec<u8>, rtype: u8 },
    GetReport { id: u32, rnum: u8, rtype: u8 },
    SetReport { id: u32, rnum: u8, rtype: u8, data: Vec<u8> },
    Other(u32),
}

fn put_str(buf: &mut [u8], at: usize, cap: usize, s: &str) {
    let b = s.as_bytes();
    let n = b.len().min(cap - 1); // keep a NUL terminator
    buf[at..at + n].copy_from_slice(&b[..n]);
}

fn rd16(b: &[u8], at: usize) -> u16 {
    u16::from_ne_bytes([b[at], b[at + 1]])
}
fn rd32(b: &[u8], at: usize) -> u32 {
    u32::from_ne_bytes(b[at..at + 4].try_into().unwrap())
}

pub fn encode_create2(info: &DeviceInfo) -> Vec<u8> {
    assert!(info.descriptor.len() <= DATA_MAX, "report descriptor too large");
    let mut b = vec![0u8; EVENT_SIZE];
    b[0..4].copy_from_slice(&UHID_CREATE2.to_ne_bytes());
    put_str(&mut b, off::NAME, 128, &info.name);
    put_str(&mut b, off::PHYS, 64, &info.phys);
    put_str(&mut b, off::UNIQ, 64, &info.uniq);
    b[off::RD_SIZE..off::RD_SIZE + 2].copy_from_slice(&(info.descriptor.len() as u16).to_ne_bytes());
    b[off::BUS..off::BUS + 2].copy_from_slice(&info.bus.to_ne_bytes());
    b[off::VENDOR..off::VENDOR + 4].copy_from_slice(&info.vendor.to_ne_bytes());
    b[off::PRODUCT..off::PRODUCT + 4].copy_from_slice(&info.product.to_ne_bytes());
    b[off::VERSION..off::VERSION + 4].copy_from_slice(&info.version.to_ne_bytes());
    b[off::COUNTRY..off::COUNTRY + 4].copy_from_slice(&0u32.to_ne_bytes());
    b[off::RD_DATA..off::RD_DATA + info.descriptor.len()].copy_from_slice(&info.descriptor);
    b
}

pub fn encode_input2(data: &[u8]) -> Vec<u8> {
    assert!(data.len() <= DATA_MAX);
    let mut b = vec![0u8; off::IN2_DATA + data.len()];
    b[0..4].copy_from_slice(&UHID_INPUT2.to_ne_bytes());
    b[off::IN2_SIZE..off::IN2_SIZE + 2].copy_from_slice(&(data.len() as u16).to_ne_bytes());
    b[off::IN2_DATA..].copy_from_slice(data);
    b
}

pub fn encode_get_report_reply(id: u32, err: u16, data: &[u8]) -> Vec<u8> {
    let mut b = vec![0u8; off::REPLY_DATA + data.len()];
    b[0..4].copy_from_slice(&UHID_GET_REPORT_REPLY.to_ne_bytes());
    b[off::REQ_ID..off::REQ_ID + 4].copy_from_slice(&id.to_ne_bytes());
    b[off::REPLY_ERR..off::REPLY_ERR + 2].copy_from_slice(&err.to_ne_bytes());
    b[off::REPLY_SIZE..off::REPLY_SIZE + 2].copy_from_slice(&(data.len() as u16).to_ne_bytes());
    b[off::REPLY_DATA..].copy_from_slice(data);
    b
}

pub fn encode_set_report_reply(id: u32, err: u16) -> Vec<u8> {
    let mut b = vec![0u8; off::REPLY_ERR + 2];
    b[0..4].copy_from_slice(&UHID_SET_REPORT_REPLY.to_ne_bytes());
    b[off::REQ_ID..off::REQ_ID + 4].copy_from_slice(&id.to_ne_bytes());
    b[off::REPLY_ERR..off::REPLY_ERR + 2].copy_from_slice(&err.to_ne_bytes());
    b
}

/// Decode one event as read from the kernel (shorter buffers are zero-extended).
pub fn decode(raw: &[u8]) -> Event {
    let mut b = [0u8; EVENT_SIZE];
    let n = raw.len().min(EVENT_SIZE);
    b[..n].copy_from_slice(&raw[..n]);
    let clamp = |s: u16| (s as usize).min(DATA_MAX);
    match rd32(&b, 0) {
        UHID_START => Event::Start { flags: u64::from_ne_bytes(b[off::START_FLAGS..off::START_FLAGS + 8].try_into().unwrap()) },
        UHID_STOP => Event::Stop,
        UHID_OPEN => Event::Open,
        UHID_CLOSE => Event::Close,
        UHID_OUTPUT => {
            let size = clamp(rd16(&b, off::OUT_SIZE));
            Event::Output { data: b[off::OUT_DATA..off::OUT_DATA + size].to_vec(), rtype: b[off::OUT_RTYPE] }
        }
        UHID_GET_REPORT => Event::GetReport { id: rd32(&b, off::REQ_ID), rnum: b[off::REQ_RNUM], rtype: b[off::REQ_RTYPE] },
        UHID_SET_REPORT => {
            let size = clamp(rd16(&b, off::SET_SIZE));
            Event::SetReport {
                id: rd32(&b, off::REQ_ID),
                rnum: b[off::REQ_RNUM],
                rtype: b[off::REQ_RTYPE],
                data: b[off::SET_DATA..off::SET_DATA + size].to_vec(),
            }
        }
        t => Event::Other(t),
    }
}

/// One virtual HID device. Dropping it destroys the device.
pub struct UhidDevice {
    file: File,
    pub info: DeviceInfo,
}

impl UhidDevice {
    pub fn create(info: DeviceInfo) -> io::Result<Self> {
        let mut file = OpenOptions::new()
            .read(true)
            .write(true)
            .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC)
            .open("/dev/uhid")?;
        write_all_once(&mut file, &encode_create2(&info))?;
        Ok(Self { file, info })
    }

    /// Send one input report (`UHID_INPUT2`).
    pub fn input(&mut self, data: &[u8]) -> io::Result<()> {
        write_all_once(&mut self.file, &encode_input2(data))
    }

    pub fn get_report_reply(&mut self, id: u32, err: u16, data: &[u8]) -> io::Result<()> {
        write_all_once(&mut self.file, &encode_get_report_reply(id, err, data))
    }

    pub fn set_report_reply(&mut self, id: u32, err: u16) -> io::Result<()> {
        write_all_once(&mut self.file, &encode_set_report_reply(id, err))
    }

    /// Next pending event, `None` if there is none right now.
    pub fn poll(&mut self) -> io::Result<Option<Event>> {
        let mut b = vec![0u8; EVENT_SIZE];
        match self.file.read(&mut b) {
            Ok(0) => Ok(None),
            Ok(n) => Ok(Some(decode(&b[..n]))),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => Ok(None),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => Ok(None),
            Err(e) => Err(e),
        }
    }

    /// The `/dev/hidrawN` node the kernel created for this device, if it exists yet.
    pub fn hidraw(&self) -> Option<PathBuf> {
        find_hidraw_by_uniq(Path::new("/sys/class/hidraw"), &self.info.uniq)
    }
}

impl Drop for UhidDevice {
    fn drop(&mut self) {
        let mut b = [0u8; 4];
        b.copy_from_slice(&UHID_DESTROY.to_ne_bytes());
        let _ = self.file.write(&b);
    }
}

fn write_all_once(f: &mut File, buf: &[u8]) -> io::Result<()> {
    // uhid consumes one event per write() and reports the full count.
    loop {
        match f.write(buf) {
            Ok(n) if n == buf.len() => return Ok(()),
            Ok(n) => return Err(io::Error::other(format!("short uhid write {n}/{}", buf.len()))),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
}

/// Find `/dev/hidrawN` whose HID device has `HID_UNIQ=<uniq>` (sysfs only, no device I/O).
pub fn find_hidraw_by_uniq(sys_hidraw: &Path, uniq: &str) -> Option<PathBuf> {
    let want = format!("HID_UNIQ={uniq}");
    let mut entries: Vec<_> = std::fs::read_dir(sys_hidraw).ok()?.flatten().collect();
    entries.sort_by_key(|e| e.file_name());
    for e in entries {
        let Ok(ue) = std::fs::read_to_string(e.path().join("device/uevent")) else { continue };
        if ue.lines().any(|l| l == want) {
            return Some(Path::new("/dev").join(e.file_name()));
        }
    }
    None
}

/// Whether the current user may open `path` read-write.
pub fn accessible(path: &Path) -> bool {
    use std::os::unix::ffi::OsStrExt;
    let Ok(c) = std::ffi::CString::new(path.as_os_str().as_bytes()) else { return false };
    // SAFETY: `c` is a valid NUL-terminated path.
    unsafe { libc::access(c.as_ptr(), libc::R_OK | libc::W_OK) == 0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn create2_layout() {
        let info = DeviceInfo {
            name: "X".into(),
            phys: "p".into(),
            uniq: "u".into(),
            bus: BUS_USB,
            vendor: 0x36B0,
            product: 0x5A11,
            version: 0x0118,
            descriptor: vec![0x06, 0x60, 0xFF],
        };
        let b = encode_create2(&info);
        assert_eq!(b.len(), 4376);
        assert_eq!(rd32(&b, 0), 11);
        assert_eq!(b[4], b'X');
        assert_eq!(b[132], b'p');
        assert_eq!(b[196], b'u');
        assert_eq!(rd16(&b, 260), 3);
        assert_eq!(rd16(&b, 262), 3);
        assert_eq!(rd32(&b, 264), 0x36B0);
        assert_eq!(rd32(&b, 268), 0x5A11);
        assert_eq!(&b[280..283], &[0x06, 0x60, 0xFF]);
    }

    #[test]
    fn output_decode() {
        let mut b = vec![0u8; EVENT_SIZE];
        b[0..4].copy_from_slice(&6u32.to_ne_bytes());
        b[4] = 0;
        b[5] = 0xBE;
        b[4100..4102].copy_from_slice(&1025u16.to_ne_bytes());
        b[4102] = UHID_OUTPUT_REPORT;
        match decode(&b) {
            Event::Output { data, rtype } => {
                assert_eq!(data.len(), 1025);
                assert_eq!(data[1], 0xBE);
                assert_eq!(rtype, 1);
            }
            e => panic!("{e:?}"),
        }
    }

    #[test]
    fn input2_and_replies() {
        let b = encode_input2(&[1, 2, 3]);
        assert_eq!(rd32(&b, 0), 12);
        assert_eq!(rd16(&b, 4), 3);
        assert_eq!(&b[6..], &[1, 2, 3]);
        let r = encode_get_report_reply(7, 5, &[]);
        assert_eq!((rd32(&r, 0), rd32(&r, 4), rd16(&r, 8), rd16(&r, 10)), (10, 7, 5, 0));
        let s = encode_set_report_reply(9, 0);
        assert_eq!((rd32(&s, 0), rd32(&s, 4), rd16(&s, 8)), (14, 9, 0));
    }
}
