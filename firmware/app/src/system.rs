//! Boot, reset, bootloader entry, watchdog and the panic handler.
//!
//! Recovery philosophy (docs/boot-recovery.md): every failure of this firmware must end in a
//! state that `tools/rollback.sh` can recover *without the owner's hands*, i.e. the stock UF2
//! download loader at 0x80020000 — never in a hang or a reset loop.
//!
//! * `BE FC` / keycode → [`enter_bootloader`]: the stock sequence (quiesce, flush + disable the
//!   caches, `fence.i`, jump to 0x80020000).
//! * `BE FF` / keycode → [`reboot`]: PPOR software reset, like stock (cold start via stage-0).
//! * **panic** → log, then [`enter_bootloader`]. Chosen over a reset: a reset re-runs the same
//!   code, so a deterministic panic (e.g. during init) would become an endless USB
//!   connect/disconnect loop that only A+S+D at plug-in can break; the loader instead shows up as
//!   the UF2 drive and `rollback.sh` / `flash_uf2.py` work at once. A transient panic costs the
//!   owner one replug (power-on → stage-0 → our app again).
//! * **watchdog** (EWDG0, 2 s, fed by the thread executor): if it ever fires, the next boot sees
//!   the EWDG reset flag in PPOR and goes straight to the loader (same reasoning: no reset loop
//!   through a hanging app). The loader itself never runs with our watchdog armed — every path
//!   into it stops EWDG0 first.

use core::fmt::Write;
use core::sync::atomic::{AtomicU32, Ordering};

use hpm_hal::pac;

use crate::console;

/// Download loader ("Zent_DLD") base. Stock `BE FC` jumps here (word 0 is the signature, which
/// decodes as two harmless compressed instructions; stage-0 itself uses +4).
pub const LOADER_ENTRY: usize = 0x8002_0000;

/// PPOR `RESET_FLAG` bit for EWDG0 (HPM5300 `ppor_reset_wdog0`, hpm_sdk). **Unverified on this
/// board** — logged at every boot so it can be confirmed on hardware.
pub const PPOR_FLAG_EWDG0: u32 = 1 << 16;
/// PPOR software reset delay in 24 MHz ticks: 6 000 000 = 250 ms (stock value).
const SOFT_RESET_TICKS: u32 = 6_000_000;
/// Watchdog timeout. Longest legitimate stall: one NOR sector erase (typ. 45 ms, max ~400 ms);
/// the flash code feeds the dog between sectors.
pub const WATCHDOG_TIMEOUT_MS: u64 = 2000;

/// Raw PPOR reset flags found at boot (before clearing), for the banner / diagnostics.
pub static RESET_FLAGS: AtomicU32 = AtomicU32::new(0);

/// Read and clear the PPOR reset flags. If the previous reset was our watchdog, go to the
/// loader instead of starting the app again. Call first thing after the console is up.
pub fn check_reset_cause() {
    let flags = pac::PPOR.reset_flag().read().0;
    RESET_FLAGS.store(flags, Ordering::Relaxed);
    pac::PPOR.reset_flag().write(|w| w.0 = flags); // W1C
    let still = pac::PPOR.reset_flag().read().0;
    crate::log!("reset flags 0x{:08x} (after clear 0x{:08x})", flags, still);
    // Only trust the bit if clearing it worked; a stuck flag must not send every boot to the
    // loader.
    if flags & PPOR_FLAG_EWDG0 != 0 && still & PPOR_FLAG_EWDG0 == 0 {
        crate::log!("previous reset was the watchdog -> entering the UF2 loader");
        console::flush_blocking();
        enter_bootloader();
    }
}

// ------------------------------------------------------------------ watchdog

/// Set to `false` to build without the watchdog (e.g. if bring-up shows EWDG0 cannot be stopped
/// before entering the loader — see the bring-up checklist).
pub const WATCHDOG_ENABLED: bool = true;

/// Subsystems that were started; [`quiesce`] only touches those (a peripheral whose clock was
/// never enabled must not be accessed).
pub static STARTED: AtomicU32 = AtomicU32::new(0);
pub const STARTED_WDG: u32 = 1 << 0;
pub const STARTED_SCAN: u32 = 1 << 1;
pub const STARTED_RGB: u32 = 1 << 2;
pub const STARTED_USB: u32 = 1 << 3;

pub fn mark_started(bit: u32) {
    STARTED.fetch_or(bit, Ordering::Relaxed);
}

/// Arm EWDG0 (32 kHz clock, reset on timeout).
pub fn watchdog_start(ewdg: hpm_hal::Peri<'static, hpm_hal::peripherals::EWDG0>) {
    if !WATCHDOG_ENABLED {
        return;
    }
    // The driver has no Drop; it is fed through the raw register below from any context.
    let mut w = hpm_hal::ewdg::Watchdog::new(ewdg, hpm_hal::ewdg::Config::default());
    w.start(embassy_time::Duration::from_millis(WATCHDOG_TIMEOUT_MS));
    mark_started(STARTED_WDG);
}

/// Refresh EWDG0 (no-op if it is not running).
#[inline]
pub fn watchdog_feed() {
    const REFRESH_KEY: u32 = 0x5A45_524F;
    if STARTED.load(Ordering::Relaxed) & STARTED_WDG != 0 {
        pac::EWDG0.wdt_refresh_reg().write(|w| w.0 = REFRESH_KEY);
    }
}

/// Stop EWDG0 before handing the CPU to code that will not feed it. If the stop does not take,
/// stretch the timeout to the maximum (~256 s) so a UF2 copy can still finish.
fn watchdog_stop() {
    let w = pac::EWDG0;
    watchdog_feed();
    w.wdt_en().write(|r| r.set_wdog_en(false));
    if w.wdt_en().read().wdog_en() {
        w.ot_rst_val().write(|r| r.set_ot_rst_val(0xFFFF));
        watchdog_feed();
    }
}

// ------------------------------------------------------------------ reset / loader

/// Busy-wait using the cycle counter (works with interrupts off).
pub fn delay_ms(ms: u32) {
    let hz = hpm_hal::sysctl::clocks().cpu0.0 as u64;
    let start = riscv::register::mcycle::read64();
    let ticks = hz / 1000 * ms as u64;
    while riscv::register::mcycle::read64().wrapping_sub(start) < ticks {}
}

/// Stop every interrupt source and bus master we started, detach from USB, and leave the PLIC
/// the way the next program expects it (we may be called from inside an interrupt handler,
/// i.e. with a claimed, uncompleted source on the preemption stack).
fn quiesce() {
    riscv::interrupt::disable();
    riscv::interrupt::disable_interrupt(riscv::interrupt::Interrupt::MachineTimer);
    let plic = pac::PLIC;
    for i in 0..4 {
        plic.targetint(0).inten(i).write(|w| w.0 = 0);
    }
    for id in 0..128u16 {
        plic.targetconfig(0).claim().write(|w| w.set_interrupt_id(id)); // complete
    }
    plic.targetconfig(0).threshold().write(|w| w.set_threshold(0));
    // PLIC.feature stays vectored + preemptive: exactly the state stock leaves for the loader.

    let started = STARTED.load(Ordering::Relaxed);
    if started & STARTED_WDG != 0 {
        watchdog_stop();
    }
    if started & STARTED_SCAN != 0 {
        // ADC trigger and mux timers, ADC interrupts, the processing timer.
        pac::PWM0.gcr().write(|w| w.0 = 0);
        pac::PWM1.gcr().write(|w| w.0 = 0);
        for adc in [pac::ADC0, pac::ADC1] {
            adc.int_en().write(|w| w.0 = 0);
            adc.seq_cfg0().write(|w| w.0 = 0);
        }
        pac::GPTMR1.irqen().write(|w| w.0 = 0);
        pac::GPTMR1.channel(0).cr().write(|w| w.0 = 0);
    }
    if started & STARTED_RGB != 0 {
        pac::HDMA.chctrl(0).ctrl().write(|w| w.0 = 0);
        pac::SPI1.ctrl().write(|w| w.0 = 0x7); // reset SPI + FIFOs
    }
    if started & STARTED_USB != 0 {
        // Run/stop = 0 drops the D+ pull-up: the host sees a detach.
        let usb = pac::USB0;
        usb.usbintr().write(|w| w.0 = 0);
        usb.usbcmd().modify(|w| w.set_rs(false));
    }
}

/// Leave for the stock UF2 download loader (stock `BE FC`, `0x00015A0C`). Never returns.
pub fn enter_bootloader() -> ! {
    enter_bootloader_after(0)
}

/// [`enter_bootloader`] with `extra_ms` more delay before the jump (bring-up probes encode
/// state in it; the watchdog is already stopped then).
pub fn enter_bootloader_after(extra_ms: u32) -> ! {
    quiesce();
    // Stock waits 1 s here: lets the host notice the detach before the loader re-enumerates.
    delay_ms(1000 + extra_ms);
    if STARTED.load(Ordering::Relaxed) & STARTED_USB != 0 {
        // Controller reset so the loader's USB stack starts from a clean state.
        let usb = pac::USB0;
        usb.usbcmd().modify(|w| w.set_rst(true));
        let mut n = 0;
        while usb.usbcmd().read().rst() && n < 1_000_000 {
            n += 1;
        }
    }
    jump_to_loader()
}

/// Andes L1 cache shutdown + jump (stock: mcctlcommand = 7 "L1D write back all", clear
/// mcache_ctl DC_EN then IC_EN, fence.i, jr 0x80020000). Runs from ILM, so turning the
/// I-cache off under our feet is harmless.
#[inline(never)]
fn jump_to_loader() -> ! {
    // SAFETY: control leaves this program for good, so no Rust state is observed afterwards.
    // The target is the stock download loader's entry in the never-written boot region, which
    // expects exactly this machine state (interrupts off, caches off — `quiesce` + the
    // sequence below reproduce what the stock firmware does before the same jump).
    unsafe {
        core::arch::asm!(
            "csrwi 0x7CC, 7",        // mcctlcommand: L1D_WB_ALL
            "csrci 0x7CA, 2",        // mcache_ctl.DC_EN = 0
            "csrci 0x7CA, 1",        // mcache_ctl.IC_EN = 0
            "fence.i",
            "jr {entry}",
            entry = in(reg) LOADER_ENTRY,
            options(noreturn),
        )
    }
}

/// PPOR software reset (stock `BE FF`): cold restart through stage-0 after 250 ms.
pub fn reboot() -> ! {
    quiesce();
    pac::PPOR.software_reset().write(|w| w.set_counter(SOFT_RESET_TICKS));
    loop {
        core::hint::spin_loop();
    }
}

// ------------------------------------------------------------------ panic

struct Blocking;
impl Write for Blocking {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        console::write_blocking(s);
        Ok(())
    }
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    riscv::interrupt::disable();
    console::record_crash(format_args!("{}", info));
    let _ = write!(Blocking, "\r\n*** PANIC: {}\r\n*** entering UF2 loader\r\n", info);
    console::flush_blocking();
    enter_bootloader()
}
