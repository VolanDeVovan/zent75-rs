#![no_main]
#![no_std]
#![feature(impl_trait_in_assoc_type)]
#![feature(abi_riscv_interrupt)]

//! ZENT75-HE board firmware: runs `zent_core::device::Keyboard` on the HPM5361.
//! Before flashing anything read docs/boot-recovery.md. Module map: firmware/README.md.

mod console;
mod flash;
// `&*(&raw const STATIC)` is deliberate in these two: it avoids references to `static mut`
// (static_mut_refs), which is what clippy would suggest instead.
#[allow(clippy::deref_addrof)]
mod rgb;
#[allow(clippy::deref_addrof)]
mod scan;
mod shared;
mod system;
mod usb;

use core::sync::atomic::Ordering;

use embassy_executor::Spawner;
use embassy_time::{Instant, Timer};
use hpm_hal as hal;
use zent_core::device::SystemAction;
use zent_core::layout::LED_COUNT;

use crate::shared::{with_kb, STORAGE};

/// TinyUF2 boot signature at 0x80040000 (`zent-link.x`, section `.app_signature`).
#[used]
#[unsafe(link_section = ".app_signature")]
static APP_BOOT_SIGNATURE: u32 = 0x0B4D_5475;

/// Config saves are debounced: a burst of `BE 03` writes becomes one erase per slot.
const SAVE_DEBOUNCE_MS: u64 = 300;
/// RGB refresh period (100 Hz; a frame takes 2.7 ms on the wire).
const RGB_PERIOD_MS: u64 = 10;

/// Stock clock tree (docs/re-peripherals.md §1): PLL0 = 24 MHz × 40 = 960 MHz,
/// PLL0CLK0/1/2 = 960/600/400 MHz, CPU = PLL0CLK0 / 2 = 480 MHz, AHB = CPU / 3 = 160 MHz.
fn clock_config() -> hal::Config {
    use hal::sysctl::{AHBDiv, ClockConfig, ClockMux, Pll};
    let mut c = hal::Config::default();
    c.sysctl.pll0 = Some(Pll { freq_in: hal::time::Hertz(960_000_000), div: (0, 3, 7) });
    c.sysctl.cpu0 = ClockConfig::new(ClockMux::PLL0CLK0, 2);
    c.sysctl.ahb_div = AHBDiv::DIV3;
    c
}

#[embassy_executor::task]
async fn console_task() {
    loop {
        console::drain();
        Timer::after_millis(1).await;
    }
}

/// Write pending config, then perform a reset / bootloader request.
fn system_action(a: SystemAction) -> ! {
    let n = STORAGE.borrow_mut().commit(0, 0, true);
    log!("system action {:?} (flushed {} slot(s))", a, n);
    console::flush_blocking();
    match a {
        SystemAction::EnterBootloader => system::enter_bootloader(),
        SystemAction::Reboot => system::reboot(),
    }
}

/// Deferred work: `Keyboard::service`, system actions, debounced flash commits.
#[embassy_executor::task]
async fn service_task() {
    loop {
        Timer::after_millis(5).await;
        let action = with_kb(|kb| {
            kb.service(&mut *STORAGE.borrow_mut());
            kb.take_system_action()
        });
        if let Some(a) = action {
            system_action(a);
        }
        // Outside the keyboard lock: erasing a sector blocks this executor for ~45 ms while
        // scanning continues in the interrupts.
        let now = Instant::now().as_millis();
        STORAGE.borrow_mut().commit(now, SAVE_DEBOUNCE_MS, false);
    }
}

#[embassy_executor::task]
async fn rgb_task() {
    let mut leds = [[0u8; 3]; LED_COUNT];
    loop {
        Timer::after_millis(RGB_PERIOD_MS).await;
        let now = Instant::now().as_millis() as u32;
        let susp = hal::pac::USB0.portsc1().read().susp();
        with_kb(|kb| {
            kb.set_usb_suspended(susp);
            kb.render_leds(now, &mut leds);
        });
        rgb::refresh(&leds);
    }
}

/// Watchdog feeding + periodic statistics on the console.
#[embassy_executor::task]
async fn health_task() {
    let cpu_mhz = hal::sysctl::clocks().cpu0.0 / 1_000_000;
    let mut tick = 0u32;
    let mut last_scans = 0u32;
    loop {
        system::watchdog_feed();
        Timer::after_millis(250).await;
        tick += 1;
        if tick % 4 == 0 {
            // BE FD tail: scans/s, engine busy per mille, uptime (stock words 499-504)
            let cyc = scan::STATS.sec_cyc.swap(0, Ordering::Relaxed);
            let busy = (cyc / (cpu_mhz * 1000).max(1)).min(1000) as u16;
            with_kb(|kb| kb.engine.second_tick(busy));
        }
        if tick % 20 != 0 {
            continue;
        }
        let s = &scan::STATS;
        let scans = s.scans.load(Ordering::Relaxed);
        let n = s.proc_n.swap(0, Ordering::Relaxed).max(1);
        let sum = s.proc_cyc_sum.swap(0, Ordering::Relaxed);
        let pmax = s.proc_cyc_max.swap(0, Ordering::Relaxed);
        let amax = s.adc_cyc_max.swap(0, Ordering::Relaxed);
        let (smin, smax) = (s.seq_min.swap(u32::MAX, Ordering::Relaxed), s.seq_max.swap(0, Ordering::Relaxed));
        // Budget per 125 µs scan: one processing run + 11 ADC captures.
        let avg_us10 = sum / n * 10 / cpu_mhz;
        let busy_pct = (sum / n + 11 * amax) * 100 / (125 * cpu_mhz);
        log!(
            "scan {}/s avg {}.{} us max {} us/scan, adc isr max {} cyc, cpu ~{}% | seq {}..{} ticks | drop {} miss {} badch {} cflct {} adc1late {}",
            (scans - last_scans) / 5,
            avg_us10 / 10,
            avg_us10 % 10,
            pmax / cpu_mhz,
            amax,
            busy_pct,
            smin,
            smax,
            s.dropped.load(Ordering::Relaxed),
            s.steps_missing.load(Ordering::Relaxed),
            s.bad_channel.load(Ordering::Relaxed),
            s.hw_conflict.load(Ordering::Relaxed),
            s.adc1_late.load(Ordering::Relaxed),
        );
        last_scans = scans;
        if tick % 240 == 0 {
            let f = scan::last_frame();
            log!("frame[0..8] {:?} log-dropped {}", &f[..8], console::dropped());
        }
    }
}

/// Load the config banks into the static keyboard (in place, see `shared::KB`).
#[inline(never)]
fn boot_keyboard(uid: [u8; 16]) {
    shared::kb_boot(|kb| {
        kb.set_serial(uid);
        kb.boot(&mut *STORAGE.borrow_mut());
        log!(
            "config: active profile {}, defaults written for empty slots: {}",
            kb.active_profile(),
            STORAGE.borrow_mut().dirty()
        );
    });
}

#[embassy_executor::main(entry = "hpm_hal::entry")]
async fn main(spawner: Spawner) -> ! {
    let p = hal::init(clock_config());
    console::init();
    log!("");
    log!(
        "ZENT75-HE zent75-rs {} (protocol version {:?})",
        env!("CARGO_PKG_VERSION"),
        zent_core::device::VERSION
    );
    let clk = hal::sysctl::clocks();
    log!("cpu {} Hz, ahb {} Hz", clk.cpu0.0, clk.ahb.0);
    system::check_reset_cause(); // may divert to the UF2 loader after a watchdog reset
    system::watchdog_start(p.EWDG0);
    shared::setup_plic();
    spawner.spawn(console_task()).unwrap();

    match flash::init() {
        Ok((total, sector, page)) => log!("flash: {} B, sector {} B, page {} B", total, sector, page),
        Err(e) => log!("flash: init FAILED {:?} - config writes disabled", e),
    }
    let uid = flash::read_uid();
    log!("uid {:02x?}", uid);

    boot_keyboard(uid);

    rgb::init();
    scan::start(p.ADC0, p.ADC1);
    log!("scan started");
    usb::start(spawner, p.USB0, p.PA24, p.PA25);
    log!("usb started");

    spawner.spawn(service_task()).unwrap();
    spawner.spawn(rgb_task()).unwrap();
    spawner.spawn(health_task()).unwrap();
    log!("init done");
    console::go_async();

    loop {
        Timer::after_secs(3600).await;
    }
}
