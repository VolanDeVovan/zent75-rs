//! Hall-sensor scan: 11 mux steps × 8 ADC inputs = 88 slots, 8000 full scans/s.
//!
//! Hardware set-up is a register-for-register copy of stock 0.0.18 (docs/re-peripherals.md §3,
//! MMIO trace `re-work/periph/mmio_decoded.txt`):
//!
//! * **PWM1** P4..P7 → PY00..PY03 generate the mux address sequence 1,2,3,4,5,9,10,11,12,13,14
//!   in hardware, one step every 1818 ticks (11.36 µs) of a 20 000-tick (125 µs) period.
//! * **PWM0** CH8REF pulses 320 ticks (2 µs) after each step boundary; **TRGM0** routes it to
//!   ADC0_STRGI and ADC1_STRGI, so both ADCs start a 16-conversion sequence at once.
//! * **ADC0** converts channels 1,3,5,7 and **ADC1** 2,4,6,11, each 4× back to back, results
//!   DMA'd into a 16-word circular buffer per ADC. 16-bit, ADC clock AHB/4 = 40 MHz.
//! * **ADC0 seq_cvc** (prio 7) fires after each sequence: we work out which step it was from
//!   the PWM0 counter (stateless, self-resynchronising), average conversions 1..3 of each input
//!   (the first is discarded, like stock) and file the 8 values into the frame. After step 10
//!   the frame is published and **GPTMR1** is kicked: its reload interrupt (prio 2, stock's
//!   "software interrupt" trick) runs `Keyboard::scan` preemptibly.
//!
//! Slot numbering: `slot = step × 8 + position`, positions interleaved
//! `[ADC0 ch1, ADC1 ch2, ADC0 ch3, ADC1 ch4, ADC0 ch5, ADC1 ch6, ADC0 ch7, ADC1 ch11]`
//! (docs/hardware.md). See [`STEP_OFFSET`] for the open one-step question.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use hpm_hal::interrupt::InterruptExt;
use hpm_hal::pac;
use zent_core::layout::{INPUTS_PER_STEP, SLOT_COUNT, STEP_COUNT};

use crate::shared;

/// **Step ↔ sample association.** The samples captured after the trigger in PWM step `k`
/// are filed as scan step `(k + STEP_OFFSET) % 11`. `0` = "a sample belongs to the address that
/// was on the mux when it was taken" (what the stock tables assume). docs/hardware.md notes the
/// stock app may be one step off; if the slot→key test on hardware shows every key reported
/// one row-step away, change this to `1` or `10` — nothing else depends on it.
pub const STEP_OFFSET: usize = 0;

/// PWM period in AHB ticks (160 MHz): 125 µs.
const PERIOD: u32 = 20_000;
/// Mux step length in ticks (step 10 is 1820 long).
const STEP_TICKS: u32 = 1818;
/// ADC trigger offset after each step boundary (2 µs settling).
const TRIGGER_TICKS: u32 = 320;
/// Expected trigger → seq_cvc-ISR latency (16 conversions at 40 MHz ≈ 9.6 µs + entry). Only
/// used to locate the step; anything within ±STEP_TICKS/2 of it decodes correctly. The real
/// value is measured and logged (`seq` min/max), tune if it drifts out of the window.
const SEQ_DONE_TICKS: u32 = 1550;

/// ADC0 then ADC1 channel per input index.
const ADC0_CH: [u8; 4] = [1, 3, 5, 7];
const ADC1_CH: [u8; 4] = [2, 4, 6, 11];

/// PWM0 compare values (register format, ticks << 4): rising edge 320, falling 420 ticks after
/// each step boundary — stock MMIO trace.
const PWM0_CMP: [u32; 22] = [
    0x01400, 0x01a40, 0x085a0, 0x08be0, 0x0f740, 0x0fd80, 0x168e0, 0x16f20, 0x1da80, 0x1e0c0, 0x24c20, 0x25260,
    0x2bdc0, 0x2c400, 0x32f60, 0x335a0, 0x3a100, 0x3a740, 0x412a0, 0x418e0, 0x48440, 0x48a80,
];
/// PWM1 compare values (ticks << 4) and the channel → compare-range assignment (stock).
const PWM1_CMP: [u32; 19] = [
    0x00000, 0x071a0, 0x0e340, 0x154e0, 0x1c680, 0x2a9c0, 0x31b60, 0x38d00, 0x3fea0, 0x47040, // P4: A0
    0x071a0, 0x154e0, 0x2a9c0, 0x38d00, 0x47040, // P5: A1
    0x154e0, 0x23820, 0x38d00, // P6: A2
    0x23820, // P7: A3
];
const PWM1_CHCFG: [(usize, u32); 4] = [(4, 0x0900_0000), (5, 0x0E0A_0000), (6, 0x110F_0000), (7, 0x1212_0000)];
const PWM_UNLOCK: u32 = 0xB038_2607;
const PWM_RLD: u32 = 0x0004_E1F0;
const PWM_PWMCFG: u32 = 0x16E0_0000;
/// TRGM0 output index of ADC0_STRGI / ADC1_STRGI and the input PWM0_CH8REF.
const TRGM_OUT_ADC0: usize = 32;
const TRGM_OUT_ADC1: usize = 33;
const TRGM_IN_PWM0_CH8REF: u32 = 0x30;

const INT_SEQ_CVC: u32 = 1 << 23;
const INT_SEQ_HW_CFLCT: u32 = 1 << 26;

// ------------------------------------------------------------------ shared buffers

#[repr(C, align(64))]
struct DmaBuf([u32; 16]);
static mut ADC0_DMA: DmaBuf = DmaBuf([0; 16]);
static mut ADC1_DMA: DmaBuf = DmaBuf([0; 16]);

/// Ping-pong frames: the ADC ISR fills `FRAMES[w]`; on step 10 it publishes `w` and switches.
static mut FRAMES: [[u16; SLOT_COUNT]; 2] = [[0; SLOT_COUNT]; 2];
static mut WRITE_IDX: usize = 0;
static READY_IDX: AtomicU32 = AtomicU32::new(0);
/// Incremented once per published frame (seqlock for the reader).
static READY_SEQ: AtomicU32 = AtomicU32::new(0);
static mut LAST_DONE: u32 = 0;
static mut LAST_GEN: u32 = 0;
static mut IN_PROCESS: bool = false;

/// Raised whenever `Keyboard::hid_generation()` moves (one per HID IN endpoint task).
pub static HID_KBD: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static HID_CONSUMER: Signal<CriticalSectionRawMutex, ()> = Signal::new();
pub static HID_MOUSE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

// ------------------------------------------------------------------ statistics

pub struct Stats {
    pub scans: AtomicU32,
    pub dropped: AtomicU32,
    pub bad_channel: AtomicU32,
    pub hw_conflict: AtomicU32,
    pub adc1_late: AtomicU32,
    pub steps_missing: AtomicU32,
    pub proc_cyc_max: AtomicU32,
    pub proc_cyc_sum: AtomicU32,
    pub proc_n: AtomicU32,
    pub adc_cyc_max: AtomicU32,
    pub seq_min: AtomicU32,
    pub seq_max: AtomicU32,
}
pub static STATS: Stats = Stats {
    scans: AtomicU32::new(0),
    dropped: AtomicU32::new(0),
    bad_channel: AtomicU32::new(0),
    hw_conflict: AtomicU32::new(0),
    adc1_late: AtomicU32::new(0),
    steps_missing: AtomicU32::new(0),
    proc_cyc_max: AtomicU32::new(0),
    proc_cyc_sum: AtomicU32::new(0),
    proc_n: AtomicU32::new(0),
    adc_cyc_max: AtomicU32::new(0),
    seq_min: AtomicU32::new(u32::MAX),
    seq_max: AtomicU32::new(0),
};
static mut STEPS_SEEN: u16 = 0;
static RUNNING: AtomicBool = AtomicBool::new(false);

#[inline(always)]
fn cycles() -> u32 {
    riscv::register::mcycle::read() as u32
}

// ------------------------------------------------------------------ init

/// Configure pins, ADCs, PWM0/PWM1/TRGM0 and GPTMR1, then start. Call after `boot_keyboard`.
pub fn start(adc0: hpm_hal::Peri<'static, hpm_hal::peripherals::ADC0>, adc1: hpm_hal::Peri<'static, hpm_hal::peripherals::ADC1>) {
    use hpm_hal::sysctl::clock_add_to_group;
    for r in [pac::resources::ADC0, pac::resources::ADC1, pac::resources::MOT0, pac::resources::TMR1] {
        clock_add_to_group(r, 0);
    }
    // ADC clock mux 0 = AHB (160 MHz), as stock.
    pac::SYSCTL.adcclk(0).write(|w| w.0 = 0);
    pac::SYSCTL.adcclk(1).write(|w| w.0 = 0);

    // Sense pads PB00, PB01, PB08..PB13 analog; mux address PY00..PY03 → PWM1_P4..P7.
    for pad in [32usize, 33, 40, 41, 42, 43, 44, 45] {
        pac::IOC.pad(pad).func_ctl().write(|w| w.0 = 0x100);
    }
    for pad in 448usize..452 {
        pac::PIOC.pad(pad).func_ctl().write(|w| w.0 = 3);
        pac::IOC.pad(pad).func_ctl().write(|w| w.0 = 0x11);
    }

    // ADC16 init + calibration (hpm-hal = hpm_sdk adc16_init sequence), then stock registers.
    let cfg = {
        let mut c = hpm_hal::adc::Config::default();
        c.resolution = hpm_hal::adc::Resolution::Bits16;
        c.clock_divider = hpm_hal::adc::ClockDivider::DIV4;
        c.disable_busywait = true;
        c
    };
    // The drivers are only used for their init + calibration; we own the registers after.
    let _ = hpm_hal::adc::Adc::new(adc0, cfg);
    let _ = hpm_hal::adc::Adc::new(adc1, cfg);
    for (adc, chans, buf) in [
        (pac::ADC0, ADC0_CH, (&raw const ADC0_DMA) as u32),
        (pac::ADC1, ADC1_CH, (&raw const ADC1_DMA) as u32),
    ] {
        adc.conv_cfg1().write(|w| w.0 = 0x153); // 16-bit, AHB/4
        adc.adc_cfg0().write(|w| w.0 = 0xA000_0000); // adc_ahb_en, sel_sync_ahb
        adc.buf_cfg0().write(|w| w.0 = 1); // wait_dis
        for &ch in &chans {
            adc.prd_cfg(ch as usize).prd_thshd_cfg().write(|w| w.0 = 0xFFFF_0000);
            adc.sample_cfg(ch as usize).write(|w| w.0 = 1);
        }
        adc.seq_dma_cfg().write(|w| w.0 = 0x2000); // dma_rst
        adc.seq_dma_cfg().write(|w| w.0 = 0);
        adc.seq_dma_addr().write(|w| w.0 = buf);
        adc.seq_dma_cfg().write(|w| w.0 = 0xF); // 16 words, circular
        adc.seq_high_cfg().write(|w| w.0 = 0);
        adc.int_sts().write(|w| w.0 = u32::MAX);
        adc.int_en().write(|w| w.0 = INT_SEQ_CVC);
        adc.seq_cfg0().write(|w| w.0 = 0xF09); // 16 entries, cont_en, hw_trig_en
        for i in 0..16 {
            let ch = chans[i / 4] as u32;
            adc.seq_que(i).write(|w| w.0 = ch | if i == 15 { 1 << 5 } else { 0 });
        }
    }

    // PWM0: ADC trigger pulses.
    let p0 = pac::PWM0;
    p0.unlk().write(|w| w.0 = PWM_UNLOCK);
    p0.rld().write(|w| w.0 = PWM_RLD);
    for (i, &v) in PWM0_CMP.iter().enumerate() {
        p0.unlk().write(|w| w.0 = PWM_UNLOCK);
        p0.cmpcfg(i).write(|w| w.0 = 0);
        p0.cmp(i).write(|w| w.0 = v);
    }
    p0.shlk().write(|w| w.0 = 0x8000_0000);
    p0.chcfg(8).write(|w| w.0 = 0x1500_0000); // CH8REF toggles on cmp 0..21
    pac::TRGM0.trgocfg(TRGM_OUT_ADC0).write(|w| w.0 = TRGM_IN_PWM0_CH8REF);
    pac::TRGM0.trgocfg(TRGM_OUT_ADC1).write(|w| w.0 = TRGM_IN_PWM0_CH8REF);

    // PWM1: mux address on P4..P7.
    let p1 = pac::PWM1;
    p1.unlk().write(|w| w.0 = PWM_UNLOCK);
    p1.rld().write(|w| w.0 = PWM_RLD);
    p1.sta().write(|w| w.0 = 0);
    for (i, &v) in PWM1_CMP.iter().enumerate() {
        p1.unlk().write(|w| w.0 = PWM_UNLOCK);
        p1.cmpcfg(i).write(|w| w.0 = 0);
        p1.cmp(i).write(|w| w.0 = v);
    }
    for (ch, cfg) in PWM1_CHCFG {
        p1.chcfg(ch).write(|w| w.0 = cfg);
        p1.pwmcfg(ch).write(|w| w.0 = PWM_PWMCFG);
    }
    p1.shlk().write(|w| w.0 = 0x8000_0000);

    // GPTMR1 ch0 = deferred "software interrupt" for frame processing (stock trick).
    let t = pac::GPTMR1;
    t.channel(0).cr().write(|w| w.0 = 0x208);
    t.channel(0).cmp(0).write_value(0);
    t.channel(0).rld().write_value(1);
    t.sr().write(|w| w.0 = 0xF);
    t.irqen().write(|w| w.0 = 1); // ch0 reload

    // Interrupts: ADC0 captures (ADC1 is polled from the same ISR), GPTMR1 processes.
    shared::set_prio(hpm_hal::interrupt::ADC0, shared::PRIO_ADC);
    shared::set_prio(hpm_hal::interrupt::GPTMR1, shared::PRIO_SCAN);
    hpm_hal::interrupt::ADC1.disable();
    unsafe {
        hpm_hal::interrupt::GPTMR1.enable();
        hpm_hal::interrupt::ADC0.enable();
    }

    crate::system::mark_started(crate::system::STARTED_SCAN);
    // Start both timers back to back (stock: not hardware-synchronised either).
    RUNNING.store(true, Ordering::Release);
    p0.sta().write(|w| w.0 = 0);
    p1.sta().write(|w| w.0 = 0);
    p0.gcr().write(|w| w.0 = 0x80); // cen
    p1.gcr().write(|w| w.0 = 0x80);
}

#[inline(always)]
fn kick_processing() {
    let cr = pac::GPTMR1.channel(0).cr();
    cr.write(|w| w.0 = 0x208 | (1 << 14)); // cntrst
    cr.write(|w| w.0 = 0x208 | (1 << 10)); // cen → reload IRQ after 2 timer ticks
}

/// Which PWM step's sequence just completed, from the PWM0 counter at ISR time.
#[inline(always)]
fn decode_step(cnt: u32) -> (usize, u32) {
    let x = (cnt + PERIOD + STEP_TICKS / 2 - TRIGGER_TICKS - SEQ_DONE_TICKS) % PERIOD;
    let k = ((x / STEP_TICKS) as usize).min(STEP_COUNT - 1);
    // Lateness of this ISR relative to the trigger of step k (for tuning SEQ_DONE_TICKS).
    let trig = k as u32 * STEP_TICKS + TRIGGER_TICKS;
    let late = (cnt + PERIOD - trig) % PERIOD;
    (k, late)
}

#[inline(always)]
fn avg3(b: &[u32; 16], input: usize) -> u16 {
    let i = input * 4;
    let s = (b[i + 1] & 0xFFFF) + (b[i + 2] & 0xFFFF) + (b[i + 3] & 0xFFFF);
    (s / 3) as u16
}

fn stat_max(a: &AtomicU32, v: u32) {
    if v > a.load(Ordering::Relaxed) {
        a.store(v, Ordering::Relaxed);
    }
}
fn stat_min(a: &AtomicU32, v: u32) {
    if v < a.load(Ordering::Relaxed) {
        a.store(v, Ordering::Relaxed);
    }
}

/// ADC0 sequence complete: capture one mux step. Highest priority, runs from ILM, ~1 µs.
#[allow(non_snake_case)]
#[hpm_hal::external_interrupt(hpm_hal::pac::interrupt::ADC0)]
unsafe fn ADC0() {
    let c0 = cycles();
    let cnt = pac::PWM0.cnt().read().cnt();
    let s0 = pac::ADC0.int_sts().read().0;
    pac::ADC0.int_sts().write(|w| w.0 = s0);
    // ADC1 runs the same-length sequence from the same trigger; it is done within a few
    // ADC clocks. Bounded wait (~2 µs).
    let mut spins = 0u32;
    while pac::ADC1.int_sts().read().0 & INT_SEQ_CVC == 0 {
        spins += 1;
        if spins > 1000 {
            STATS.adc1_late.fetch_add(1, Ordering::Relaxed);
            break;
        }
    }
    let s1 = pac::ADC1.int_sts().read().0;
    pac::ADC1.int_sts().write(|w| w.0 = s1);
    if (s0 | s1) & INT_SEQ_HW_CFLCT != 0 {
        STATS.hw_conflict.fetch_add(1, Ordering::Relaxed);
    }

    let (k, late) = decode_step(cnt);
    stat_min(&STATS.seq_min, late);
    stat_max(&STATS.seq_max, late);

    let b0 = &*(&raw const ADC0_DMA.0);
    let b1 = &*(&raw const ADC1_DMA.0);
    // Sanity: the last word of each buffer must be the last channel of the sequence.
    if (b0[15] >> 20) & 0x1F != ADC0_CH[3] as u32 || (b1[15] >> 20) & 0x1F != ADC1_CH[3] as u32 {
        STATS.bad_channel.fetch_add(1, Ordering::Relaxed);
    }
    let w = WRITE_IDX;
    let step = (k + STEP_OFFSET) % STEP_COUNT;
    let frame = &mut *(&raw mut FRAMES[w]);
    let base = step * INPUTS_PER_STEP;
    for i in 0..4 {
        frame[base + 2 * i] = avg3(b0, i);
        frame[base + 2 * i + 1] = avg3(b1, i);
    }
    STEPS_SEEN |= 1 << k;

    if k == STEP_COUNT - 1 {
        let seen = STEPS_SEEN.count_ones();
        STEPS_SEEN = 0;
        if seen != STEP_COUNT as u32 {
            STATS.steps_missing.fetch_add(1, Ordering::Relaxed);
        }
        // Publish only plausible frames: if step decoding ever breaks (e.g. the PWM0 counter
        // reads a constant) we must not run Keyboard::scan once per *step* and starve the
        // executor; the scan then stops visibly (console `scan 0/s`, `miss` counting up).
        if seen < STEP_COUNT as u32 - 1 {
            stat_max(&STATS.adc_cyc_max, cycles().wrapping_sub(c0));
            hpm_hal::interrupt::ADC0.complete();
            return;
        }
        READY_IDX.store(w as u32, Ordering::Relaxed);
        READY_SEQ.fetch_add(1, Ordering::Release);
        WRITE_IDX = w ^ 1;
        kick_processing();
    }
    stat_max(&STATS.adc_cyc_max, cycles().wrapping_sub(c0));
    hpm_hal::interrupt::ADC0.complete();
}

/// Deferred frame processing (stock's GPTMR1 trick). Preemptible by ADC0 and USB0.
#[allow(non_snake_case)]
#[hpm_hal::external_interrupt(hpm_hal::pac::interrupt::GPTMR1)]
unsafe fn GPTMR1() {
    let t = pac::GPTMR1;
    t.channel(0).cr().write(|w| w.0 = 0x208); // stop
    t.sr().write(|w| w.0 = 1);
    // With PLIC preemption the gateway never re-delivers a claimed source, so this handler
    // cannot nest into itself. Guard anyway (cheap): a nested entry just returns, the running
    // instance loops until it has consumed the newest frame.
    if IN_PROCESS {
        hpm_hal::interrupt::GPTMR1.complete();
        return;
    }
    IN_PROCESS = true;
    riscv::interrupt::enable(); // nesting: higher PLIC priorities may preempt from here

    let mut frame = [0u16; SLOT_COUNT];
    loop {
        let seq = READY_SEQ.load(Ordering::Acquire);
        if seq == LAST_DONE {
            break;
        }
        let idx = READY_IDX.load(Ordering::Relaxed) as usize;
        frame.copy_from_slice(&*(&raw const FRAMES[idx]));
        if READY_SEQ.load(Ordering::Acquire) != seq {
            continue; // a newer frame was published while copying: take that one
        }
        let missed = seq.wrapping_sub(LAST_DONE).wrapping_sub(1);
        if missed != 0 && LAST_DONE != 0 {
            STATS.dropped.fetch_add(missed, Ordering::Relaxed);
        }
        LAST_DONE = seq;

        let c0 = cycles();
        let kb = shared::kb_from_scan_isr();
        kb.scan(&frame);
        let gen = kb.hid_generation();
        let dc = cycles().wrapping_sub(c0);
        stat_max(&STATS.proc_cyc_max, dc);
        STATS.proc_cyc_sum.fetch_add(dc, Ordering::Relaxed);
        STATS.proc_n.fetch_add(1, Ordering::Relaxed);
        STATS.scans.fetch_add(1, Ordering::Relaxed);
        if gen != LAST_GEN {
            LAST_GEN = gen;
            HID_KBD.signal(());
            HID_CONSUMER.signal(());
            HID_MOUSE.signal(());
        }
    }

    riscv::interrupt::disable();
    IN_PROCESS = false;
    hpm_hal::interrupt::GPTMR1.complete();
}

/// Snapshot of the latest published frame (for diagnostics).
pub fn last_frame() -> [u16; SLOT_COUNT] {
    let idx = READY_IDX.load(Ordering::Relaxed) as usize;
    unsafe { *(&raw const FRAMES[idx]) }
}
