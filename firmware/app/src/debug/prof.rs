//! Statistical CPU profiler (cargo feature `profiler`): GPTMR0 interrupts at ~10 kHz (a period unrelated to
//! the 8 kHz scan and its 11 mux steps, so samples do not alias onto fixed points of the frame)
//! and records the interrupted PC. `tools/profile.py` starts a capture, reads the samples (debug
//! commands, see `debug`) and maps them to functions of the ELF. It sees everything below
//! [`plic::PRIO_PROF`], i.e. all but the ADC capture handler. Costs nothing until started.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering::Relaxed};

use hpm_hal::interrupt::{self, InterruptExt};
use hpm_hal::pac;

use crate::plic;

/// Samples per capture (~0.2 s at the sample rate below); `tools/profile.py` sums captures.
/// 8 KB of AHB SRAM; most of the rest holds the engine's height tables (`kb`).
pub const SAMPLES: usize = 2048;
/// GPTMR0 reload in 24 MHz ticks: 2399 → ~10.004 kHz (2399 is prime).
const RELOAD: u32 = 2399;

/// In AHB SRAM (DLM is full); not zeroed at boot, but only indices below `COUNT` are read.
// SAFETY: `.ahb_sram` is a NOLOAD RAM section of `zent-link.x`; atomics need no initial value.
#[unsafe(link_section = ".ahb_sram")]
static BUF: [AtomicU32; SAMPLES] = [const { AtomicU32::new(0) }; SAMPLES];
static COUNT: AtomicU32 = AtomicU32::new(0);
static ACTIVE: AtomicBool = AtomicBool::new(false);

/// Set up GPTMR0 ch0 (stopped). Call once at init.
pub fn init() {
    hpm_hal::sysctl::clock_add_to_group(pac::resources::TMR0, 0);
    let t = pac::GPTMR0;
    t.channel(0).cr().write(|w| w.0 = 0);
    t.channel(0).cmp(0).write_value(0);
    t.channel(0).rld().write_value(RELOAD);
    t.sr().write(|w| w.0 = 0xF);
    t.irqen().write(|w| w.0 = 1); // ch0 reload
    plic::enable(interrupt::GPTMR0, plic::PRIO_PROF);
}

/// Start a new capture (discarding the previous one).
pub fn start() {
    ACTIVE.store(false, Relaxed);
    COUNT.store(0, Relaxed);
    let cr = pac::GPTMR0.channel(0).cr();
    cr.write(|w| w.0 = 1 << 14); // cntrst
    cr.write(|w| w.0 = 1 << 10); // cen
    ACTIVE.store(true, Relaxed);
}

/// Copy samples from index `from` on into `out` as LE words; returns (samples taken so far,
/// samples copied).
pub fn samples(from: u32, out: &mut [u8]) -> (u32, usize) {
    let taken = COUNT.load(Relaxed).min(SAMPLES as u32);
    let from = from.min(taken) as usize;
    let copied = (taken as usize - from).min(out.len() / 4);
    for (bytes, sample) in out.as_chunks_mut::<4>().0.iter_mut().zip(&BUF[from..from + copied]) {
        *bytes = sample.load(Relaxed).to_le_bytes();
    }
    (taken, copied)
}

#[allow(non_snake_case)]
#[hpm_hal::external_interrupt(hpm_hal::pac::interrupt::GPTMR0)]
fn GPTMR0() {
    let pc = riscv::register::mepc::read() as u32;
    let t = pac::GPTMR0;
    t.sr().write(|w| w.0 = 1);
    if ACTIVE.load(Relaxed) {
        let i = COUNT.load(Relaxed) as usize;
        if i < SAMPLES {
            BUF[i].store(pc, Relaxed);
            COUNT.store(i as u32 + 1, Relaxed);
        } else {
            ACTIVE.store(false, Relaxed);
            t.channel(0).cr().write(|w| w.0 = 0);
        }
    }
    interrupt::GPTMR0.complete();
}
