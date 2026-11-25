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
use embassy_usb::driver::{
    EndpointAddress, EndpointAllocError, EndpointIn, EndpointOut, EndpointType, Event, Unsupported,
};
use embassy_usb::{Builder, Handler};
use hpm_hal::pac;
use hpm_hal::peripherals;
use hpm_hal::usb::{EndpointState, UsbDriver};
use static_cell::StaticCell;
use zent_core::hid::{descriptor, HidReports};
use zent_core::protocol::REPORT_SIZE;

use crate::scan::HidSignal;
use crate::{kb, log, plic, scan, StoreRef};

/// USB0 binding. Not `hpm_hal::bind_interrupts!`: its handler completes through hpm-hal's
/// `complete()`, a read-modify-write of the PLIC claim register — the read *claims* any
/// higher-priority pending source (ADC0), which is then never serviced or completed and
/// stops the scan for good. See `plic::complete`.
#[derive(Copy, Clone)]
struct Irqs;
// SAFETY: `Binding` promises that the driver's handler runs whenever USB0 fires. The `USB0`
// vector below does exactly that (and nothing else but the PLIC completion).
unsafe impl hpm_hal::interrupt::typelevel::Binding<hpm_hal::interrupt::typelevel::USB0, hpm_hal::usb::InterruptHandler<peripherals::USB0>>
    for Irqs
{
}

#[allow(non_snake_case)]
#[hpm_hal::external_interrupt(hpm_hal::pac::interrupt::USB0)]
fn USB0() {
    use hpm_hal::interrupt::typelevel::Handler;
    // SAFETY: called from the USB0 vector only, which is the contract of `on_interrupt`.
    unsafe {
        <hpm_hal::usb::InterruptHandler<peripherals::USB0> as Handler<hpm_hal::interrupt::typelevel::USB0>>::on_interrupt()
    };
    plic::complete(hpm_hal::interrupt::USB0);
}

type Driver = AlwaysPowered<UsbDriver<'static, peripherals::USB0>>;
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

// ------------------------------------------------------------------ VBUS

/// The keyboard is bus-powered and has no VBUS sense pin (VBUS/session valid are forced in
/// PHY_CTRL0), so — like stock CherryUSB — power is simply always present.
///
/// hpm-hal's bus instead reports `OTGSC.ASV`: it reads 0 at the first `poll` (the override
/// has not settled yet) and drops again when `Bus::enable` sets OTGSC.VD (VBUS discharge).
/// embassy-usb answers `PowerRemoved` with `Bus::disable`, which resets the controller and
/// the PHY — after which ASV never comes back and the board stays off the bus (seen on
/// hardware with bring-up probes 7 and 8). This wrapper reports `PowerDetected` once and
/// drops every later power event.
struct AlwaysPowered<D>(D);

impl<'a, D: embassy_usb::driver::Driver<'a>> embassy_usb::driver::Driver<'a> for AlwaysPowered<D> {
    type EndpointOut = D::EndpointOut;
    type EndpointIn = D::EndpointIn;
    type ControlPipe = D::ControlPipe;
    type Bus = AlwaysPoweredBus<D::Bus>;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointOut, EndpointAllocError> {
        self.0.alloc_endpoint_out(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<Self::EndpointIn, EndpointAllocError> {
        self.0.alloc_endpoint_in(ep_type, ep_addr, max_packet_size, interval_ms)
    }

    fn start(self, control_max_packet_size: u16) -> (Self::Bus, Self::ControlPipe) {
        let (bus, pipe) = self.0.start(control_max_packet_size);
        (AlwaysPoweredBus { bus, powered: false }, pipe)
    }
}

struct AlwaysPoweredBus<B> {
    bus: B,
    powered: bool,
}

impl<B: embassy_usb::driver::Bus> embassy_usb::driver::Bus for AlwaysPoweredBus<B> {
    async fn enable(&mut self) {
        self.bus.enable().await
    }

    async fn disable(&mut self) {
        self.bus.disable().await
    }

    async fn poll(&mut self) -> Event {
        if !self.powered {
            self.powered = true;
            return Event::PowerDetected;
        }
        loop {
            match self.bus.poll().await {
                Event::PowerDetected | Event::PowerRemoved => continue,
                e => return e,
            }
        }
    }

    fn endpoint_set_enabled(&mut self, ep_addr: EndpointAddress, enabled: bool) {
        self.bus.endpoint_set_enabled(ep_addr, enabled)
    }

    fn endpoint_set_stalled(&mut self, ep_addr: EndpointAddress, stalled: bool) {
        self.bus.endpoint_set_stalled(ep_addr, stalled)
    }

    fn endpoint_is_stalled(&mut self, ep_addr: EndpointAddress) -> bool {
        self.bus.endpoint_is_stalled(ep_addr)
    }

    fn force_reset(&mut self) -> Result<(), Unsupported> {
        self.bus.force_reset()
    }

    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        self.bus.remote_wakeup().await
    }
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
            scan::signal_hid();
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
                    kb::with(|kb| kb.set_host_leds(leds));
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
pub fn suspended() -> bool {
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

/// Wait until the host configured us and `ep` is open. hpm-hal opens the endpoints on
/// SET_CONFIGURATION without waking tasks parked in `wait_enabled` (only a bus reset wakes
/// them), so a task that got there first would sleep forever — the keyboard enumerated but
/// sent nothing and ignored vendor frames. `CONFIGURED` is set after the endpoints are open.
async fn endpoint_ready(ep: &mut impl embassy_usb::driver::Endpoint) {
    while !CONFIGURED.load(Ordering::Relaxed) {
        Timer::after_millis(1).await;
    }
    ep.wait_enabled().await;
}

/// One input report: up to 16 bytes.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Report {
    data: [u8; 16],
    len: usize,
}

impl Report {
    fn new(bytes: &[u8]) -> Self {
        let mut data = [0; 16];
        data[..bytes.len()].copy_from_slice(bytes);
        Self { data, len: bytes.len() }
    }
}

/// Send `current()` on `ep` whenever `signal` fires and the report differs from the one the
/// host last got (stock sends on change only).
async fn send_on_change(mut ep: EpIn, signal: &HidSignal, current: impl Fn(&HidReports) -> Report) -> ! {
    let mut sent = None;
    loop {
        endpoint_ready(&mut ep).await;
        signal.wait().await;
        loop {
            let r = kb::with(|kb| current(kb.hid()));
            if sent == Some(r) {
                break;
            }
            wake_host().await;
            if ep.write(&r.data[..r.len]).await.is_err() {
                sent = None; // endpoint disabled: resend once it is back
                break;
            }
            sent = Some(r);
        }
    }
}

#[embassy_executor::task]
async fn keyboard_task(ep: EpIn) -> ! {
    send_on_change(ep, &scan::HID_KBD, |h| {
        if BOOT_PROTOCOL.load(Ordering::Relaxed) {
            Report::new(&h.boot_report())
        } else {
            Report::new(&h.keyboard)
        }
    })
    .await
}

#[embassy_executor::task]
async fn consumer_task(ep: EpIn) -> ! {
    send_on_change(ep, &scan::HID_CONSUMER, |h| Report::new(&h.consumer)).await
}

#[embassy_executor::task]
async fn mouse_task(ep: EpIn) -> ! {
    send_on_change(ep, &scan::HID_MOUSE, |h| Report::new(&h.mouse_report())).await
}

type VendorIn = Mutex<NoopRawMutex, EpIn>;

/// Vendor OUT 0x03 → `Keyboard::vendor_in` → reply on IN 0x82.
#[embassy_executor::task]
async fn vendor_out_task(mut ep: EpOut, tx: &'static VendorIn, store: StoreRef) -> ! {
    let mut rx = [0u8; REPORT_SIZE];
    let mut reply = [0u8; REPORT_SIZE];
    loop {
        endpoint_ready(&mut ep).await;
        let n = match ep.read(&mut rx).await {
            Ok(n) => n,
            Err(_) => continue,
        };
        reply.fill(0);
        let replied = kb::with(|kb| kb.vendor_in(&rx[..n], &mut *store.borrow_mut(), &mut reply));
        if replied.is_some() {
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
        if kb::with(|kb| kb.vendor_poll(&mut frame)).is_some() {
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
    store: StoreRef,
) {
    // The controller's queue heads and transfer descriptors. Plain `.bss`: all RAM of this
    // image is DLM, which is not behind the D-cache (no `.noncacheable` placement needed).
    static EP_STATE: EndpointState = EndpointState::new();

    crate::system::mark_started(crate::system::STARTED_USB);
    let driver = AlwaysPowered(UsbDriver::new(usb0, Irqs, dm, dp, hpm_hal::usb::Config::default(), &EP_STATE));
    plic::set_priority(hpm_hal::interrupt::USB0, plic::PRIO_USB);

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
    spawner.spawn(vendor_out_task(vendor_out.unwrap(), vin, store)).unwrap();
    spawner.spawn(vendor_poll_task(vin)).unwrap();
}
