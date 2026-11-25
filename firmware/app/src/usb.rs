//! USB HS device reproducing the stock ZENT75-HE (docs/re-peripherals.md §5): VID 0x36B0,
//! PID 0x357E, strings "Wob Lab" / "ZENT  75 HE " / "20240728", four HID interfaces with the
//! stock report descriptors and endpoint numbers, all interrupt endpoints `bInterval = 1`
//! (125 µs at high speed; embassy-usb writes `interval_ms` verbatim into bInterval).
//!
//! | IF | EP            | Report                                    |
//! |----|---------------|-------------------------------------------|
//! | 0  | 0x81 IN 64    | keyboard: 16-byte NKRO (8-byte boot report if the host selects boot protocol) |
//! | 1  | 0x83 IN 4 (*) | consumer: 24-usage bitmap (3 bytes)       |
//! | 2  | 0x82 IN 1024, 0x03 OUT 1024 | vendor 0xFF60/0x61 (configurator protocol) |
//! | 3  | 0x84 IN 4 (*) | mouse: buttons 1..5, X, Y (3 bytes)       |
//!
//! Unlike stock, the IN endpoints are independent (each has its own task), so a 1024-byte vendor
//! frame never delays a key report.
//!
//! Deliberate differences from stock: (*) wMaxPacketSize 4 instead of 3 (see
//! [`SMALL_EP_MPS`]), `bmAttributes` = 0xA0 (embassy-usb sets the reserved
//! bit 7 the spec requires; stock sends 0x20), GET_PROTOCOL reports the protocol actually
//! selected (stock always answers 0), and in boot protocol the keyboard sends the 8-byte boot
//! report (stock keeps sending NKRO, which BIOSes cannot parse).

use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::NoopRawMutex;
use embassy_sync::mutex::Mutex;
use embassy_time::Timer;
use embassy_usb::control::{InResponse, OutResponse, Recipient, Request, RequestType};
use embassy_usb::driver::{Endpoint as _, EndpointAddress, EndpointIn, EndpointOut};
use embassy_usb::{Builder, Handler};
use hpm_hal::pac;
use hpm_hal::peripherals;
use hpm_hal::usb::{EndpointState, UsbDriver};
use static_cell::StaticCell;
use zent_core::hid::descriptor;
use zent_core::protocol::REPORT_SIZE;

use crate::shared::{self, with_kb, STORAGE};
use crate::{log, scan};

/// USB0 binding. Not `hpm_hal::bind_interrupts!`: its handler completes through hpm-hal's
/// `complete()`, a read-modify-write of the PLIC claim register — the read *claims* any
/// higher-priority pending source (ADC0), which is then never serviced or completed and
/// stops the scan for good. See `shared::plic_complete`.
#[derive(Copy, Clone)]
struct Irqs;
unsafe impl hpm_hal::interrupt::typelevel::Binding<hpm_hal::interrupt::typelevel::USB0, hpm_hal::usb::InterruptHandler<peripherals::USB0>>
    for Irqs
{
}

#[allow(non_snake_case)]
#[hpm_hal::external_interrupt(hpm_hal::pac::interrupt::USB0)]
unsafe fn USB0() {
    use hpm_hal::interrupt::typelevel::Handler;
    <hpm_hal::usb::InterruptHandler<peripherals::USB0> as Handler<hpm_hal::interrupt::typelevel::USB0>>::on_interrupt();
    shared::plic_complete(hpm_hal::interrupt::USB0);
}

type Driver = UsbDriver<'static, peripherals::USB0>;
type EpIn = <Driver as embassy_usb::driver::Driver<'static>>::EndpointIn;
type EpOut = <Driver as embassy_usb::driver::Driver<'static>>::EndpointOut;

pub const VID: u16 = 0x36B0;
pub const PID: u16 = 0x357E;
/// Stock serial string (fixed, not the chip UID): keeps host-side driver bindings identical.
pub const SERIAL: &str = "20240728";
/// wMaxPacketSize of the 3-byte consumer/mouse endpoints. Stock declares 3; hpm-hal appends a
/// zero-length packet to every IN transfer whose length equals wMaxPacketSize, which would
/// reach the host as an empty HID report after each 3-byte report. 4 avoids that.
const SMALL_EP_MPS: u16 = 4;

/// (class, subclass, protocol), bcdHID, report descriptor.
type InterfaceDef = ((u8, u8, u8), u16, &'static [u8]);
/// The four interfaces in descriptor order.
const INTERFACES: [InterfaceDef; 4] = [
    ((0x03, 0x01, 0x01), 0x0111, &descriptor::KEYBOARD),
    ((0x03, 0x00, 0x00), 0x0100, &descriptor::CONSUMER),
    ((0x03, 0x00, 0x00), 0x0111, &descriptor::VENDOR),
    ((0x03, 0x01, 0x02), 0x0111, &descriptor::MOUSE),
];

fn hid_descriptor(bcd: u16, report_len: usize) -> [u8; 7] {
    [bcd as u8, (bcd >> 8) as u8, 0x00, 0x01, 0x22, report_len as u8, (report_len >> 8) as u8]
}

// ------------------------------------------------------------------ control requests

static BOOT_PROTOCOL: AtomicBool = AtomicBool::new(false);
static REMOTE_WAKEUP: AtomicBool = AtomicBool::new(false);
static CONFIGURED: AtomicBool = AtomicBool::new(false);
static IDLE: [AtomicU8; 4] = [const { AtomicU8::new(0) }; 4];

struct HidHandler {
    hid_desc: [[u8; 9]; 4],
}

impl HidHandler {
    fn new() -> Self {
        let mut hid_desc = [[0u8; 9]; 4];
        for (i, (_, bcd, rd)) in INTERFACES.iter().enumerate() {
            hid_desc[i][0] = 9;
            hid_desc[i][1] = 0x21;
            hid_desc[i][2..].copy_from_slice(&hid_descriptor(*bcd, rd.len()));
        }
        Self { hid_desc }
    }
}

const GET_REPORT: u8 = 0x01;
const GET_IDLE: u8 = 0x02;
const GET_PROTOCOL: u8 = 0x03;
const SET_REPORT: u8 = 0x09;
const SET_IDLE: u8 = 0x0A;
const SET_PROTOCOL: u8 = 0x0B;

impl Handler for HidHandler {
    fn reset(&mut self) {
        BOOT_PROTOCOL.store(false, Ordering::Relaxed);
        CONFIGURED.store(false, Ordering::Relaxed);
    }

    fn configured(&mut self, configured: bool) {
        CONFIGURED.store(configured, Ordering::Relaxed);
        log!("usb: configured={} speed={}", configured, pac::USB0.portsc1().read().pspd());
        if configured {
            // Resend current state on (re)configuration.
            scan::HID_KBD.signal(());
            scan::HID_CONSUMER.signal(());
            scan::HID_MOUSE.signal(());
        }
    }

    fn remote_wakeup_enabled(&mut self, enabled: bool) {
        REMOTE_WAKEUP.store(enabled, Ordering::Relaxed);
    }

    fn control_out(&mut self, req: Request, data: &[u8]) -> Option<OutResponse> {
        let iface = req.index as usize;
        if req.recipient != Recipient::Interface || iface >= INTERFACES.len() {
            return None;
        }
        if req.request_type != RequestType::Class {
            return None;
        }
        match req.request {
            SET_REPORT => {
                // Stock: a 1-byte output report is the keyboard LED byte (only Caps used).
                if iface == 0 && data.len() == 1 {
                    let leds = data[0];
                    with_kb(|kb| kb.set_host_leds(leds));
                }
                Some(OutResponse::Accepted)
            }
            SET_IDLE => {
                IDLE[iface].store((req.value >> 8) as u8, Ordering::Relaxed);
                Some(OutResponse::Accepted)
            }
            SET_PROTOCOL => {
                if iface == 0 {
                    BOOT_PROTOCOL.store(req.value == 0, Ordering::Relaxed);
                    scan::HID_KBD.signal(());
                }
                Some(OutResponse::Accepted)
            }
            _ => Some(OutResponse::Rejected),
        }
    }

    fn control_in<'a>(&'a mut self, req: Request, buf: &'a mut [u8]) -> Option<InResponse<'a>> {
        let iface = req.index as usize;
        if req.recipient != Recipient::Interface || iface >= INTERFACES.len() {
            return None;
        }
        match (req.request_type, req.request) {
            (RequestType::Standard, Request::GET_DESCRIPTOR) => match (req.value >> 8) as u8 {
                0x22 => Some(InResponse::Accepted(INTERFACES[iface].2)),
                0x21 => Some(InResponse::Accepted(&self.hid_desc[iface])),
                _ => Some(InResponse::Rejected),
            },
            (RequestType::Class, GET_REPORT) => {
                buf[0] = 0; // stock: one zero byte
                Some(InResponse::Accepted(&buf[..1]))
            }
            (RequestType::Class, GET_IDLE) => {
                buf[0] = IDLE[iface].load(Ordering::Relaxed);
                Some(InResponse::Accepted(&buf[..1]))
            }
            (RequestType::Class, GET_PROTOCOL) => {
                buf[0] = if iface == 0 && BOOT_PROTOCOL.load(Ordering::Relaxed) { 0 } else { 1 };
                Some(InResponse::Accepted(&buf[..1]))
            }
            (RequestType::Class, _) => Some(InResponse::Rejected),
            _ => None,
        }
    }
}

// ------------------------------------------------------------------ tasks

/// Host suspended us (bus idle ≥ 3 ms)? `PORTSC1.SUSP`, polled like stock.
fn suspended() -> bool {
    pac::USB0.portsc1().read().susp()
}

/// Stock remote wakeup: force port resume for 10 ms.
async fn wake_host() {
    if !REMOTE_WAKEUP.load(Ordering::Relaxed) || !suspended() {
        return;
    }
    log!("usb: remote wakeup");
    pac::USB0.portsc1().modify(|w| w.set_fpr(true));
    Timer::after_millis(10).await;
    pac::USB0.portsc1().modify(|w| w.set_fpr(false));
}

#[embassy_executor::task]
async fn usb_run(mut dev: embassy_usb::UsbDevice<'static, Driver>) -> ! {
    dev.run().await
}

#[embassy_executor::task]
async fn keyboard_task(mut ep: EpIn) -> ! {
    let mut last = [0xFFu8; 16];
    let mut last_boot = false;
    loop {
        ep.wait_enabled().await;
        scan::HID_KBD.wait().await;
        loop {
            let h = with_kb(|kb| *kb.hid());
            let boot = BOOT_PROTOCOL.load(Ordering::Relaxed);
            let r = h.keyboard;
            if r == last && boot == last_boot {
                break;
            }
            wake_host().await;
            let res = if boot { ep.write(&h.boot_report()).await } else { ep.write(&r).await };
            if res.is_err() {
                last = [0xFF; 16];
                break;
            }
            last = r;
            last_boot = boot;
        }
    }
}

#[embassy_executor::task]
async fn consumer_task(mut ep: EpIn) -> ! {
    let mut last = [0xFFu8; 3];
    loop {
        ep.wait_enabled().await;
        scan::HID_CONSUMER.wait().await;
        loop {
            let r = with_kb(|kb| kb.hid().consumer);
            if r == last {
                break;
            }
            wake_host().await;
            if ep.write(&r).await.is_err() {
                last = [0xFF; 3];
                break;
            }
            last = r;
        }
    }
}

#[embassy_executor::task]
async fn mouse_task(mut ep: EpIn) -> ! {
    let mut last = [0xFFu8; 3];
    loop {
        ep.wait_enabled().await;
        scan::HID_MOUSE.wait().await;
        loop {
            let r = with_kb(|kb| kb.hid().mouse_report());
            if r == last {
                break;
            }
            wake_host().await;
            if ep.write(&r).await.is_err() {
                last = [0xFF; 3];
                break;
            }
            last = r;
        }
    }
}

type VendorIn = Mutex<NoopRawMutex, EpIn>;

/// Vendor OUT 0x03 → `Keyboard::vendor_in` → reply on IN 0x82.
#[embassy_executor::task]
async fn vendor_out_task(mut ep: EpOut, tx: &'static VendorIn) -> ! {
    let mut rx = [0u8; REPORT_SIZE];
    let mut reply = [0u8; REPORT_SIZE];
    loop {
        ep.wait_enabled().await;
        let n = match ep.read(&mut rx).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        reply.fill(0);
        let r = with_kb(|kb| {
            let mut st = STORAGE.borrow_mut();
            kb.vendor_in(&rx[..n], &mut *st, &mut reply)
        });
        if r.is_some() {
            let _ = tx.lock().await.write(&reply).await;
        }
    }
}

/// Unsolicited vendor frames (`BE FD` live stream, `BE C0` profile notice) on IN 0x82.
#[embassy_executor::task]
async fn vendor_poll_task(tx: &'static VendorIn) -> ! {
    let mut frame = [0u8; REPORT_SIZE];
    loop {
        Timer::after_millis(2).await;
        if !CONFIGURED.load(Ordering::Relaxed) {
            continue;
        }
        frame.fill(0);
        if with_kb(|kb| kb.vendor_poll(&mut frame)).is_some() {
            let _ = tx.lock().await.write(&frame).await;
        }
    }
}

/// Build the device and spawn its tasks.
pub fn start(
    spawner: Spawner,
    usb0: hpm_hal::Peri<'static, peripherals::USB0>,
    dm: hpm_hal::Peri<'static, peripherals::PA24>,
    dp: hpm_hal::Peri<'static, peripherals::PA25>,
) {
    #[unsafe(link_section = ".noncacheable")]
    static EP_STATE: EndpointState = EndpointState::new();

    crate::system::mark_started(crate::system::STARTED_USB);
    let driver = UsbDriver::new(usb0, Irqs, dm, dp, hpm_hal::usb::Config::default(), &EP_STATE);
    shared::set_prio(hpm_hal::interrupt::USB0, shared::PRIO_USB);

    let mut config = embassy_usb::Config::new(VID, PID);
    config.bcd_usb = embassy_usb::UsbVersion::Two;
    config.device_class = 0;
    config.device_sub_class = 0;
    config.device_protocol = 0;
    config.composite_with_iads = false;
    config.device_release = 0x0002;
    config.manufacturer = Some("Wob Lab");
    config.product = Some("ZENT  75 HE ");
    config.serial_number = Some(SERIAL);
    config.max_power = 500;
    config.supports_remote_wakeup = true;
    config.max_packet_size_0 = 64;

    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 64]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 0]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 128]> = StaticCell::new();
    static HANDLER: StaticCell<HidHandler> = StaticCell::new();
    static VENDOR_IN: StaticCell<VendorIn> = StaticCell::new();

    let mut b = Builder::new(
        driver,
        config,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 64]),
        MSOS_DESC.init([0; 0]),
        CONTROL_BUF.init([0; 128]),
    );
    b.handler(HANDLER.init(HidHandler::new()));

    let ep = |a: u8| Some(EndpointAddress::from(a));
    let mut eps_in: [Option<EpIn>; 4] = [None, None, None, None];
    let mut vendor_out: Option<EpOut> = None;
    for (i, ((class, sub, proto), bcd, rd)) in INTERFACES.iter().enumerate() {
        let mut f = b.function(*class, *sub, *proto);
        let mut ifc = f.interface();
        let mut alt = ifc.alt_setting(*class, *sub, *proto, None);
        alt.descriptor(0x21, &hid_descriptor(*bcd, rd.len()));
        match i {
            0 => eps_in[0] = Some(alt.endpoint_interrupt_in(ep(0x81), 64, 1)),
            1 => eps_in[1] = Some(alt.endpoint_interrupt_in(ep(0x83), SMALL_EP_MPS, 1)),
            2 => {
                eps_in[2] = Some(alt.endpoint_interrupt_in(ep(0x82), 1024, 1));
                vendor_out = Some(alt.endpoint_interrupt_out(ep(0x03), 1024, 1));
            }
            _ => eps_in[3] = Some(alt.endpoint_interrupt_in(ep(0x84), SMALL_EP_MPS, 1)),
        }
    }
    let dev = b.build();
    let [kbd, cons, vin, mouse] = eps_in.map(|e| e.unwrap());
    let vin = VENDOR_IN.init(Mutex::new(vin));

    spawner.spawn(usb_run(dev)).unwrap();
    spawner.spawn(keyboard_task(kbd)).unwrap();
    spawner.spawn(consumer_task(cons)).unwrap();
    spawner.spawn(mouse_task(mouse)).unwrap();
    spawner.spawn(vendor_out_task(vendor_out.unwrap(), vin)).unwrap();
    spawner.spawn(vendor_poll_task(vin)).unwrap();
}
