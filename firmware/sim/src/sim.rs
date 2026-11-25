//! The simulated board: `zent_core::device::Keyboard` + sensor model + storage + virtual HID.
//!
//! [`Sim::tick`] advances simulated time by a number of scans (8 kHz); the real-time driver in
//! [`run`] calls it once per millisecond with however many scans are due. Tests can drive it
//! directly and stay deterministic.

use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::time::{Duration, Instant};

use zent_core::device::{Keyboard, SystemAction};
use zent_core::hid::HidState;
use zent_core::layout::{KEY_COUNT, LED_COUNT, SCAN_HZ};
use zent_core::protocol::REPORT_SIZE;

use crate::command::{Command, HELP};
use crate::hid;
use crate::keynames;
use crate::sensor::{FULL_TRAVEL_MM, SensorModel};
use crate::store::DirStorage;
use crate::uhid::{self, DeviceInfo, Event, UhidDevice};

/// UID reported by `BE 1D`.
pub const SERIAL: [u8; 16] = *b"ZENT75-SIM\0\0\0\0\0\x01";
const EIO: u16 = 5;
const SCANS_PER_MS: u64 = SCAN_HZ as u64 / 1000;

#[derive(Debug, Clone)]
pub struct Options {
    pub noise: u16,
    pub slew_mm_s: f32,
    pub seed: u64,
    /// Create the virtual vendor device on /dev/uhid.
    pub uhid: bool,
    /// Also create a keyboard device that types into the desktop session.
    pub hid_keyboard: bool,
    /// Run `sudo -n chmod 0666` on the hidraw node if it is not accessible.
    pub sudo_chmod: bool,
    /// How long the board stays away after `BE FC`.
    pub bootloader_delay: Duration,
    pub verbose: bool,
    /// Tag that makes this instance's devices unique (HID_UNIQ).
    pub instance: String,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            noise: 30,
            slew_mm_s: 200.0,
            seed: 1,
            uhid: false,
            hid_keyboard: false,
            sudo_chmod: false,
            bootloader_delay: Duration::from_secs(2),
            verbose: false,
            instance: format!("zent75-sim-{}", std::process::id()),
        }
    }
}

pub struct Sim {
    pub opts: Options,
    pub kb: Box<Keyboard>,
    pub storage: DirStorage,
    pub sensor: SensorModel,
    pub leds: [[u8; 3]; LED_COUNT],
    vendor: Option<UhidDevice>,
    keyboard: Option<UhidDevice>,
    vendor_node: Option<PathBuf>,
    node_checked: Option<Instant>,
    scans: u64,
    boot_scan: u64,
    taps: Vec<(u64, usize)>,
    hid_gen: u32,
    last_hid: HidState,
    offline_until: Option<Instant>,
    /// Number of vendor reports answered (for tests and logs).
    pub vendor_requests: u64,
}

pub fn log(msg: impl AsRef<str>) {
    println!("zent-sim: {}", msg.as_ref());
}

impl Sim {
    pub fn new(opts: Options, storage: DirStorage) -> std::io::Result<Self> {
        let sensor = SensorModel::new(opts.seed, opts.noise, opts.slew_mm_s);
        let mut s = Self {
            opts,
            kb: Box::new(Keyboard::new(SERIAL)),
            storage,
            sensor,
            leds: [[0; 3]; LED_COUNT],
            vendor: None,
            keyboard: None,
            vendor_node: None,
            node_checked: None,
            scans: 0,
            boot_scan: 0,
            taps: Vec::new(),
            hid_gen: 0,
            last_hid: HidState::default(),
            offline_until: None,
            vendor_requests: 0,
        };
        s.create_devices()?;
        s.boot();
        Ok(s)
    }

    fn create_devices(&mut self) -> std::io::Result<()> {
        if self.opts.uhid {
            self.vendor = Some(UhidDevice::create(DeviceInfo {
                name: hid::VENDOR_NAME.into(),
                phys: "zent75-sim/vendor".into(),
                uniq: format!("{}-vendor", self.opts.instance),
                bus: uhid::BUS_USB,
                vendor: hid::VID,
                product: hid::SIM_PID,
                version: hid::SIM_VERSION,
                descriptor: hid::VENDOR_DESCRIPTOR.to_vec(),
            })?);
            log(format!(
                "uhid vendor device created ({:04X}:{:04X} \"{}\")",
                hid::VID,
                hid::SIM_PID,
                hid::VENDOR_NAME
            ));
        }
        if self.opts.uhid && self.opts.hid_keyboard {
            self.keyboard = Some(UhidDevice::create(DeviceInfo {
                name: hid::KEYBOARD_NAME.into(),
                phys: "zent75-sim/keyboard".into(),
                uniq: format!("{}-keyboard", self.opts.instance),
                bus: uhid::BUS_USB,
                vendor: hid::VID,
                product: hid::SIM_PID,
                version: hid::SIM_VERSION,
                descriptor: hid::KEYBOARD_DESCRIPTOR.to_vec(),
            })?);
            log("uhid keyboard device created (types into this session!)");
        }
        self.vendor_node = None;
        self.node_checked = None;
        Ok(())
    }

    fn destroy_devices(&mut self) {
        if self.vendor.take().is_some() {
            log("uhid vendor device destroyed");
        }
        self.keyboard = None;
        self.vendor_node = None;
    }

    /// Power-up: fresh `Keyboard`, banks loaded from storage. Keys that are physically held
    /// stay held (that is how "key held at power-up" is reproduced).
    pub fn boot(&mut self) {
        *self.kb = Keyboard::new(SERIAL);
        self.kb.boot(&mut self.storage);
        self.boot_scan = self.scans;
        self.hid_gen = self.kb.hid_generation();
        self.last_hid = HidState::default();
        self.send_keyboard_reports(true);
        log(format!(
            "boot: profile {} (state {})",
            self.kb.active_profile(),
            self.storage.dir().display()
        ));
    }

    pub fn online(&self) -> bool {
        self.offline_until.is_none()
    }

    pub fn scans(&self) -> u64 {
        self.scans
    }

    /// Milliseconds since the last boot, in simulated time.
    pub fn uptime_ms(&self) -> u32 {
        ((self.scans - self.boot_scan) / SCANS_PER_MS) as u32
    }

    pub fn vendor_node(&self) -> Option<&PathBuf> {
        self.vendor_node.as_ref()
    }

    /// Advance by `n` scans, then do the per-millisecond housekeeping.
    pub fn tick(&mut self, n: u64) {
        if let Some(t) = self.offline_until {
            if Instant::now() < t {
                return;
            }
            self.offline_until = None;
            log("bootloader: done, re-enumerating as if re-flashed");
            if let Err(e) = self.create_devices() {
                log(format!("ERROR: cannot recreate uhid device: {e}"));
            }
            self.boot();
        }
        for _ in 0..n {
            self.sensor.step();
            let s = self.sensor.samples();
            self.kb.scan(&s);
            self.scans += 1;
            if !self.taps.is_empty() {
                let now = self.scans;
                let sensor = &mut self.sensor;
                self.taps.retain(|&(at, key)| {
                    if at <= now {
                        sensor.set_target(key, 0.0);
                    }
                    at > now
                });
            }
        }
        self.housekeeping();
    }

    fn housekeeping(&mut self) {
        self.poll_uhid();
        if self.kb.hid_generation() != self.hid_gen {
            self.hid_gen = self.kb.hid_generation();
            let h = *self.kb.hid();
            if h != self.last_hid {
                log(format!("hid {}", hid::describe(&h)));
                self.send_keyboard_reports(false);
                self.last_hid = h;
            }
        }
        let mut out = [0u8; REPORT_SIZE];
        while self.kb.vendor_poll(&mut out).is_some() {
            self.send_vendor(&out);
        }
        self.kb.service(&mut self.storage);
        if let Some(a) = self.kb.take_system_action() {
            self.system_action(a);
        }
        self.check_node();
    }

    fn system_action(&mut self, a: SystemAction) {
        match a {
            SystemAction::Reboot => {
                log("system: reboot requested");
                self.boot();
            }
            SystemAction::EnterBootloader => self.enter_bootloader(),
        }
    }

    pub fn enter_bootloader(&mut self) {
        log(format!(
            "system: enter bootloader (device gone for {:?})",
            self.opts.bootloader_delay
        ));
        self.destroy_devices();
        self.offline_until = Some(Instant::now() + self.opts.bootloader_delay);
    }

    fn send_vendor(&mut self, report: &[u8; REPORT_SIZE]) {
        if let Some(v) = self.vendor.as_mut()
            && let Err(e) = v.input(report)
        {
            log(format!("uhid input failed: {e}"));
        }
    }

    fn send_keyboard_reports(&mut self, all: bool) {
        let Some(k) = self.keyboard.as_mut() else {
            return;
        };
        let h = *self.kb.hid();
        let old = self.last_hid;
        let mut res = Ok(());
        if all || h.keyboard != old.keyboard {
            res = res.and(k.input(&hid::keyboard_report(&h)));
        }
        if all || h.consumer != old.consumer {
            res = res.and(k.input(&hid::consumer_report(&h)));
        }
        if all || h.system != old.system {
            res = res.and(k.input(&hid::system_report(&h)));
        }
        if all || h.mouse_buttons != old.mouse_buttons {
            res = res.and(k.input(&hid::mouse_report(&h)));
        }
        if let Err(e) = res {
            log(format!("keyboard report failed: {e}"));
        }
    }

    /// Feed one vendor output report (with or without the hidraw report-id byte) to the core.
    pub fn vendor_report(&mut self, data: &[u8]) -> Option<[u8; REPORT_SIZE]> {
        let data = if data.len() == REPORT_SIZE + 1 && data[0] == 0 {
            &data[1..]
        } else {
            data
        };
        if self.opts.verbose && data.len() >= 2 && data[1] != 0 {
            log(format!("vendor <- {:02x?}", &data[..data.len().min(12)]));
        }
        self.vendor_requests += 1;
        let mut out = [0u8; REPORT_SIZE];
        self.kb
            .vendor_in(data, &mut self.storage, &mut out)
            .map(|_| out)
    }

    fn poll_uhid(&mut self) {
        loop {
            let ev = match self.vendor.as_mut().map(|v| v.poll()) {
                Some(Ok(Some(ev))) => ev,
                Some(Err(e)) => {
                    log(format!("uhid read failed: {e}"));
                    break;
                }
                _ => break,
            };
            match ev {
                Event::Output { data, .. } => {
                    if let Some(r) = self.vendor_report(&data) {
                        self.send_vendor(&r);
                    }
                }
                Event::SetReport {
                    id, rtype, data, ..
                } => {
                    let ok = rtype == uhid::UHID_OUTPUT_REPORT;
                    let reply = if ok { self.vendor_report(&data) } else { None };
                    if let Some(v) = self.vendor.as_mut() {
                        let _ = v.set_report_reply(id, if ok { 0 } else { EIO });
                    }
                    if let Some(r) = reply {
                        self.send_vendor(&r);
                    }
                }
                Event::GetReport { id, .. } => {
                    if let Some(v) = self.vendor.as_mut() {
                        let _ = v.get_report_reply(id, EIO, &[]);
                    }
                }
                Event::Open => log("vendor hidraw opened"),
                Event::Close => log("vendor hidraw closed"),
                Event::Start { .. } | Event::Stop | Event::Other(_) => {}
            }
        }
        while let Some(Ok(Some(ev))) = self.keyboard.as_mut().map(|v| v.poll()) {
            match ev {
                Event::Output { ref data, .. }
                    if data.len() >= 2 && data[0] == hid::RID_KEYBOARD =>
                {
                    self.kb.set_host_leds(data[1]);
                }
                Event::SetReport { id, ref data, .. }
                    if data.len() >= 2 && data[0] == hid::RID_KEYBOARD =>
                {
                    self.kb.set_host_leds(data[1]);
                    if let Some(k) = self.keyboard.as_mut() {
                        let _ = k.set_report_reply(id, 0);
                    }
                }
                Event::SetReport { id, .. } => {
                    if let Some(k) = self.keyboard.as_mut() {
                        let _ = k.set_report_reply(id, EIO);
                    }
                }
                Event::GetReport { id, .. } => {
                    if let Some(k) = self.keyboard.as_mut() {
                        let _ = k.get_report_reply(id, EIO, &[]);
                    }
                }
                _ => {}
            }
        }
    }

    /// Locate the hidraw node once it appears; fix or report its permissions.
    fn check_node(&mut self) {
        let Some(v) = self.vendor.as_ref() else {
            return;
        };
        if self.vendor_node.is_some() {
            return;
        }
        let now = Instant::now();
        if self
            .node_checked
            .is_some_and(|t| now - t < Duration::from_millis(50))
        {
            return;
        }
        self.node_checked = Some(now);
        let Some(node) = v.hidraw() else { return };
        log(format!("hidraw {}", node.display()));
        if !uhid::accessible(&node) {
            if self.opts.sudo_chmod {
                let n = node.clone();
                std::thread::spawn(move || {
                    let ok = std::process::Command::new("sudo")
                        .args(["-n", "chmod", "0666"])
                        .arg(&n)
                        .status()
                        .is_ok_and(|s| s.success());
                    log(format!(
                        "sudo -n chmod 0666 {}: {}",
                        n.display(),
                        if ok { "done" } else { "FAILED" }
                    ));
                });
            } else {
                log(format!(
                    "{} is not accessible for this user; run: sudo chmod 0666 {} (or see firmware/sim/README.md)",
                    node.display(),
                    node.display()
                ));
            }
        }
        self.vendor_node = Some(node);
    }

    pub fn render_leds(&mut self) {
        let now = self.uptime_ms();
        self.kb.render_leds(now, &mut self.leds);
    }

    /// Execute one control command (everything except `sleep`/`quit`, which belong to the
    /// caller). Returns the text to print.
    pub fn exec(&mut self, cmd: Command) -> Result<String, String> {
        match cmd {
            Command::Press { key, mm } => {
                self.taps.retain(|&(_, k)| k != key);
                self.sensor.set_target(key, mm);
                Ok(String::new())
            }
            Command::Release { key: Some(key) } => {
                self.taps.retain(|&(_, k)| k != key);
                self.sensor.set_target(key, 0.0);
                Ok(String::new())
            }
            Command::Release { key: None } => {
                self.taps.clear();
                self.sensor.release_all();
                Ok(String::new())
            }
            Command::Tap { key, ms } => {
                self.sensor.set_target(key, FULL_TRAVEL_MM);
                self.taps.retain(|&(_, k)| k != key);
                self.taps.push((self.scans + ms as u64 * SCANS_PER_MS, key));
                Ok(String::new())
            }
            Command::State => Ok(self.state()),
            Command::Leds => {
                self.render_leds();
                let mut s = String::from("leds");
                for c in &self.leds {
                    let _ = write!(s, " {:02x}{:02x}{:02x}", c[0], c[1], c[2]);
                }
                Ok(s)
            }
            Command::HoldAtBoot(keys) => {
                for &k in &keys {
                    self.sensor.set_now(k, FULL_TRAVEL_MM);
                }
                self.boot();
                let names: Vec<&str> = keys.iter().map(|&k| keynames::name(k)).collect();
                Ok(format!(
                    "rebooted with {} held",
                    if names.is_empty() {
                        "nothing".into()
                    } else {
                        names.join(" ")
                    }
                ))
            }
            Command::Reboot => {
                self.boot();
                Ok(String::new())
            }
            Command::Bootloader => {
                self.enter_bootloader();
                Ok(String::new())
            }
            Command::Help => Ok(HELP.to_string()),
            Command::Sleep(_) | Command::Quit => Err("not handled here".into()),
        }
    }

    pub fn state(&self) -> String {
        let mut s = String::new();
        let _ = writeln!(
            s,
            "mode {}",
            if self.online() { "app" } else { "bootloader" }
        );
        let _ = writeln!(s, "profile {}", self.kb.active_profile());
        let _ = writeln!(s, "uptime_ms {}", self.uptime_ms());
        let _ = writeln!(s, "engine_ready {}", self.kb.engine.ready() as u8);
        let _ = writeln!(s, "hid {}", hid::describe(self.kb.hid()));
        if let Some(n) = &self.vendor_node {
            let _ = writeln!(s, "hidraw {}", n.display());
        }
        for k in 0..KEY_COUNT {
            let phys = self.sensor.travel(k);
            let tgt = self.sensor.target(k);
            let t = self.kb.engine.travel[k];
            // idle keys show a few µm of noise; list only moving keys or real travel
            if phys > 0.0 || tgt > 0.0 || t >= 100 {
                let _ = writeln!(
                    s,
                    "key {k} {} mm={phys:.3} target={tgt:.3} raw={} travel_um={t}",
                    keynames::name(k),
                    self.kb.engine.raw[k]
                );
            }
        }
        s.pop();
        s
    }
}

impl Drop for Sim {
    fn drop(&mut self) {
        self.destroy_devices();
    }
}

/// A command from the socket or a script, with the channel for its reply.
pub struct Request {
    pub cmd: Command,
    pub reply: Sender<Result<String, String>>,
}

/// Real-time loop: scans at 8 kHz in 1 ms batches, LEDs at 60 Hz, commands between batches.
/// Returns when `quit` arrives or `stop` is set.
pub fn run(sim: &mut Sim, rx: &Receiver<Request>, stop: &AtomicBool) {
    let start = Instant::now();
    let mut done: u64 = 0;
    let mut next_led = start;
    let led_period = Duration::from_micros(16_667);
    'outer: loop {
        if stop.load(Ordering::Relaxed) {
            log("signal: stopping");
            break;
        }
        while let Ok(req) = rx.try_recv() {
            match req.cmd {
                Command::Quit => {
                    let _ = req.reply.send(Ok(String::new()));
                    log("quit");
                    break 'outer;
                }
                c => {
                    let r = sim.exec(c);
                    let _ = req.reply.send(r);
                }
            }
        }
        let now = Instant::now();
        let due = ((now - start).as_nanos() * SCAN_HZ as u128 / 1_000_000_000) as u64;
        let mut n = due.saturating_sub(done);
        if n > 100 * SCANS_PER_MS {
            log(format!(
                "behind real time by {} ms, skipping",
                n / SCANS_PER_MS
            ));
            done = due - SCANS_PER_MS;
            n = SCANS_PER_MS;
        }
        sim.tick(n);
        done += n;
        if now >= next_led {
            sim.render_leds();
            next_led = now + led_period;
        }
        let next =
            start + Duration::from_nanos((done + SCANS_PER_MS) * 1_000_000_000 / SCAN_HZ as u64);
        let now = Instant::now();
        if next > now {
            std::thread::sleep(next - now);
        }
    }
}
