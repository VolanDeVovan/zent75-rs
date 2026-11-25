#![no_main]
#![no_std]
#![feature(impl_trait_in_assoc_type)]
#![feature(abi_riscv_interrupt)]

//! ZENT75-HE board firmware: runs `zent_core::device::Keyboard` on the HPM5361.
//! Before flashing anything read docs/boot-recovery.md. Module map: firmware/README.md.

mod console;
mod debug;
mod flash;
mod kb;
mod plic;
mod rgb;
mod scan;
mod system;
mod usb;

use core::cell::RefCell;
use core::sync::atomic::Ordering;

use embassy_executor::Spawner;
use embassy_time::{Instant, Timer};
use hpm_hal as hal;
use static_cell::ConstStaticCell;
use zent_core::device::SystemAction;
use zent_core::layout::{LED_COUNT, STEP_COUNT};

use crate::flash::{RomNor, Store};

/// TinyUF2 boot signature at 0x80040000 (`zent-link.x`, section `.app_signature`).
// SAFETY: the section holds exactly this word (asserted by the linker script); stage-0 only
// reads it.
#[used]
#[unsafe(link_section = ".app_signature")]
static APP_BOOT_SIGNATURE: u32 = 0x0B4D_5475;

/// Config-bank storage, shared by the thread-mode tasks only (a `RefCell`, handed out once
/// by `main`). Initialised at link time: its two bank images never pass over the stack.
static STORE: ConstStaticCell<RefCell<Store>> = ConstStaticCell::new(RefCell::new(Store::new(RomNor::new())));
/// The thread-mode handle to [`STORE`].
pub type StoreRef = &'static RefCell<Store>;

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
fn system_action(a: SystemAction, store: StoreRef) -> ! {
    let n = store.borrow_mut().commit(0, 0, true);
    log!("system action {:?} (flushed {} slot(s))", a, n);
    console::flush_blocking();
    match a {
        SystemAction::EnterBootloader => system::enter_bootloader(),
        SystemAction::Reboot => system::reboot(),
    }
}

/// Deferred work: `Keyboard::service`, system actions, debounced flash commits.
#[embassy_executor::task]
async fn service_task(store: StoreRef) {
    let mut reported = store.borrow().stats();
    loop {
        Timer::after_millis(5).await;
        let action = kb::with(|kb| {
            kb.service(&mut *store.borrow_mut());
            kb.take_system_action()
        });
        if let Some(a) = action {
            system_action(a, store);
        }
        // Outside the keyboard lock: erasing a sector blocks this executor for ~45 ms while
        // scanning continues in the interrupts.
        let now = Instant::now().as_millis();
        store.borrow_mut().commit(now, SAVE_DEBOUNCE_MS, false);
        let stats = store.borrow().stats();
        if stats != reported {
            reported = stats;
            log!("flash: {} slot(s) saved, {} failed, {} refused since boot", stats.commits, stats.errors, stats.refused);
        }
    }
}

#[embassy_executor::task]
async fn rgb_task(mut rgb: rgb::Rgb) {
    let mut leds = [[0u8; 3]; LED_COUNT];
    loop {
        Timer::after_millis(RGB_PERIOD_MS).await;
        let now = Instant::now().as_millis() as u32;
        let susp = usb::suspended();
        kb::with(|kb| {
            kb.set_usb_suspended(susp);
            kb.render_leds(now, &mut leds);
        });
        rgb.refresh(&leds);
    }
}

/// Watchdog feed every 250 ms, the `BE FD` tail every second, statistics every 5 s.
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
            let cyc = scan::STATS.busy_cyc.swap(0, Ordering::Relaxed);
            let busy = (cyc / (cpu_mhz * 1000)).min(1000) as u16;
            kb::with(|kb| kb.second_tick(busy));
        }
        if tick % 20 == 0 {
            let scans = scan::STATS.scans.load(Ordering::Relaxed);
            log_stats((scans.wrapping_sub(last_scans)) / 5, cpu_mhz);
            last_scans = scans;
        }
    }
}

/// One statistics line (format: firmware/README.md, "Console output"). Resets the per-report
/// maxima.
fn log_stats(scans_per_s: u32, cpu_mhz: u32) {
    let s = &scan::STATS;
    let take = |a: &core::sync::atomic::AtomicU32, reset: u32| a.swap(reset, Ordering::Relaxed);
    let count = |a: &core::sync::atomic::AtomicU32| a.load(Ordering::Relaxed);
    let avg = take(&s.proc_cyc_sum, 0) / take(&s.proc_n, 0).max(1);
    let (max, adc_max) = (take(&s.proc_cyc_max, 0), take(&s.adc_cyc_max, 0));
    let (seq_min, seq_max) = (take(&s.seq_min, u32::MAX), take(&s.seq_max, 0));
    // Share of the 125 µs scan period: one processing run + 11 ADC captures.
    let cpu_pct = (avg + STEP_COUNT as u32 * adc_max) * 100 / (125 * cpu_mhz);
    let avg_us10 = avg * 10 / cpu_mhz;
    log!(
        "scan {}/s avg {}.{} max {} us | adc isr max {} cyc | cpu {}% | seq {}..{} ticks | drop {} miss {} late {} badch {} cflct {} adc1late {} logdrop {}",
        scans_per_s,
        avg_us10 / 10,
        avg_us10 % 10,
        max / cpu_mhz,
        adc_max,
        cpu_pct,
        seq_min,
        seq_max,
        count(&s.dropped),
        count(&s.steps_missing),
        count(&s.late_steps),
        count(&s.bad_channel),
        count(&s.hw_conflict),
        count(&s.adc1_late),
        console::dropped(),
    );
}

/// Load the config banks into the static keyboard (in place, see `kb`). Nothing else uses
/// the keyboard yet: the scan interrupt and the USB tasks start afterwards.
#[inline(never)]
fn boot_keyboard(uid: [u8; 16], store: StoreRef) {
    kb::attach_height_tables();
    let active = kb::with(|kb| {
        kb.boot(uid, &mut *store.borrow_mut());
        kb.active_profile()
    });
    log!("config: active profile {}, defaults written for empty slots: {}", active, store.borrow().dirty());
}

#[embassy_executor::main(entry = "hpm_hal::entry")]
async fn main(spawner: Spawner) -> ! {
    let p = hal::init(clock_config());
    console::init();
    log!("");
    log!("ZENT75-HE zent75-rs {} (protocol version {:?})", env!("CARGO_PKG_VERSION"), zent_core::device::VERSION);
    debug::crash::report_previous();
    let clk = hal::sysctl::clocks();
    log!("cpu {} Hz, ahb {} Hz", clk.cpu0.0, clk.ahb.0);
    system::check_reset_cause(); // may divert to the UF2 loader after a watchdog reset
    system::watchdog_start(p.EWDG0);
    plic::setup();
    spawner.spawn(console_task()).unwrap();

    let store: StoreRef = STORE.take();
    match store.borrow_mut().nor().init(p.XPI0) {
        Ok(()) => log!("flash: 1 MB NOR ready"),
        Err(e) => log!("flash: init FAILED {:?} - config writes disabled", e),
    }
    let uid = flash::read_uid();
    log!("uid {:02x?}", uid);
    boot_keyboard(uid, store);

    let rgb = rgb::Rgb::new();
    scan::start(p.ADC0, p.ADC1);
    #[cfg(feature = "profiler")]
    debug::prof::init();
    usb::start(spawner, p.USB0, p.PA24, p.PA25, store);

    spawner.spawn(service_task(store)).unwrap();
    spawner.spawn(rgb_task(rgb)).unwrap();
    spawner.spawn(health_task()).unwrap();
    log!("init done");
    console::go_async();

    core::future::pending().await
}
