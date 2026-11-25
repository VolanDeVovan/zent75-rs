//! 85 × WS2812 on SPI1 MOSI (PA29), streamed by HDMA — stock 0.0.18 register values
//! (docs/re-peripherals.md §4), but non-blocking: we start a frame and only check for
//! completion at the next refresh instead of busy-waiting 2.7 ms.
//!
//! **Timing.** SPI1 kernel clock is set explicitly to the 24 MHz XTAL (stock never wrote it and
//! relied on that default), `SCLK_DIV = 0` → SCLK = 24 MHz / 2 = **12 MHz**, 83.3 ns per SPI bit.
//! One 16-bit SPI word encodes one WS2812 bit (1.333 µs, spec 1.25 ± 0.6 µs):
//! `1` → `0xFFC0` (10 high bits = 833 ns, spec T1H 580..1000 ns),
//! `0` → `0xE000` (3 high bits = 250 ns, spec T0H 220..380 ns).
//! A frame is 85 × 24 words + one zero word = 2041 words ≈ 2.72 ms; MOSI then rests low
//! (reset/latch ≥ 280 µs is guaranteed by the ≥ 10 ms refresh interval).
//! Wire order per LED: G, R, B, MSB first (core renders `[r, g, b]`).

use hpm_hal::pac;
use zent_core::layout::LED_COUNT;

/// Words per frame: 24 per LED + trailing zero (stock buffer layout).
pub const WORDS: usize = LED_COUNT * 24 + 1;
const ONE: u16 = 0xFFC0;
const ZERO: u16 = 0xE000;

/// SPI1 per-frame register values (stock).
const SPI_TIMING: u32 = 0x0000_3B00; // sclk_div 0, cs2sclk 3, csht 11
const SPI_TRANS_FMT: u32 = 0x0001_0F03; // 16-bit words, CPOL=1 CPHA=1
/// Stock uses transmode 5 (write, dummy, read) with an unbounded read phase and restarts the
/// controller every frame. Kept identical because it is what is proven on this board; write-
/// only (transmode 1: `0x011F8000`) would be the clean alternative if MOSI misbehaves.
const SPI_TRANS_CTRL: u32 = 0x051F_81FF;
const SPI_CTRL_START: u32 = 0x0100_0017; // cs_en, txdmaen, reset SPI + both FIFOs
const SPI_CMD_GO: u32 = 0xFF;
/// DMAMUX request number of SPI1 TX (HPM5300 map, as stock).
const DMAMUX_SPI1_TX: u32 = 0x2B;
/// HDMA ch0 control: 16-bit src/dst, dst fixed + handshake, half-count IRQ masked (stock).
const DMA_CTRL: u32 = 0x0025_2010;
const DMA_CH: usize = 0;

#[repr(C, align(64))]
struct Buf([u16; WORDS]);
/// DLM is not behind the D-cache, so the DMA sees CPU writes without a flush.
static mut BUF: Buf = Buf([0; WORDS]);
static mut BUSY: bool = false;

pub fn init() {
    use pac::sysctl::vals::ClockMux;
    hpm_hal::sysctl::clock_add_to_group(pac::resources::SPI1, 0);
    hpm_hal::sysctl::clock_add_to_group(pac::resources::HDMA, 0);
    pac::SYSCTL.clock(pac::clocks::SPI1).modify(|w| {
        w.set_mux(ClockMux::CLK_24M);
        w.set_div(0);
    });
    while pac::SYSCTL.clock(pac::clocks::SPI1).read().loc_busy() {}
    crate::system::mark_started(crate::system::STARTED_RGB);
    pac::IOC.pad(pac::pins::PA29).func_ctl().write(|w| w.0 = 5); // SPI1_MOSI
    let s = pac::SPI1;
    s.timing().write(|w| w.0 = SPI_TIMING);
    s.trans_fmt().write(|w| w.0 = SPI_TRANS_FMT);
    pac::DMAMUX.muxcfg(DMA_CH).write(|w| w.0 = 0x8000_0000 | DMAMUX_SPI1_TX);
    pac::HDMA.chctrl(DMA_CH).ctrl().write(|w| w.0 = DMA_CTRL);
    // All LEDs off at boot.
    let off = [[0u8; 3]; LED_COUNT];
    let _ = refresh(&off);
}

fn dma_idle() -> bool {
    let h = pac::HDMA;
    h.chctrl(DMA_CH).ctrl().read().0 & 1 == 0 || h.inttcsts().read().0 & (1 << DMA_CH) != 0
}

/// Start sending `leds` if the previous frame has finished. Returns false if still busy.
pub fn refresh(leds: &[[u8; 3]; LED_COUNT]) -> bool {
    unsafe {
        if BUSY && !dma_idle() {
            return false;
        }
        let h = pac::HDMA;
        h.chctrl(DMA_CH).ctrl().write(|w| w.0 = DMA_CTRL); // disable
        let buf = &mut *(&raw mut BUF.0);
        let mut i = 0;
        for &[r, g, b] in leds.iter() {
            for byte in [g, r, b] {
                for bit in (0..8).rev() {
                    buf[i] = if byte >> bit & 1 != 0 { ONE } else { ZERO };
                    i += 1;
                }
            }
        }
        buf[WORDS - 1] = 0;

        let s = pac::SPI1;
        s.trans_ctrl().write(|w| w.0 = SPI_TRANS_CTRL);
        s.ctrl().write(|w| w.0 = SPI_CTRL_START);
        s.wr_trans_cnt().write(|w| w.0 = (WORDS - 1) as u32);
        s.rd_trans_cnt().write(|w| w.0 = u32::MAX);
        s.ctrl().write(|w| w.0 = SPI_CTRL_START);
        s.cmd().write(|w| w.0 = SPI_CMD_GO);

        let ch = h.chctrl(DMA_CH);
        ch.src_addr().write_value(buf.as_ptr() as u32);
        ch.dst_addr().write_value(s.data().as_ptr() as u32);
        ch.tran_size().write(|w| w.0 = WORDS as u32);
        ch.llpointer().write(|w| w.0 = 0);
        ch.chan_req_ctrl().write(|w| w.0 = 0);
        let m = 1 << DMA_CH;
        h.inthalfsts().write(|w| w.0 = m);
        h.inttcsts().write(|w| w.0 = m);
        h.intabortsts().write(|w| w.0 = m);
        h.interrsts().write(|w| w.0 = m);
        ch.ctrl().write(|w| w.0 = DMA_CTRL | 1); // enable
        BUSY = true;
    }
    true
}
