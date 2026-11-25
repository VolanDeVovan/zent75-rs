#![no_main]
#![no_std]
#![feature(impl_trait_in_assoc_type)]

//! Firmware skeleton for the HOVEY WORKS ZENT75-HE.
//!
//! Proves the toolchain, the memory map (app at 0x80040000 with the boot
//! signature) and the embassy runtime. Hall scan, USB HID and the protocol are
//! the next milestones in docs/firmware-plan.md. Before flashing anything read
//! docs/boot-recovery.md.

use embassy_executor::Spawner;
use embassy_time::Timer;
use hal::Peri;
use hal::gpio::{AnyPin, Flex};
use hpm_hal as hal;
use panic_halt as _;

/// TinyUF2 boot signature. The stock bootloader checks that the first 32-bit
/// word of the application image (0x80040000) equals this value, then jumps to
/// the entry at 0x80040004. `link-app.x` puts this word at 0x80040000 (section
/// `.app_signature`, INSERTed before `.text`) and shifts the reset handler to
/// +4. `#[used]` + the linker `KEEP` stop it from being garbage-collected.
#[used]
#[unsafe(link_section = ".app_signature")]
static APP_BOOT_SIGNATURE: u32 = 0x0B4D_5475;

/// Toggle a GPIO forever. Pin choice is a placeholder for the skeleton; the real
/// status-LED / RGB wiring will come from docs/hardware.md.
#[embassy_executor::task]
async fn blink(pin: Peri<'static, AnyPin>) {
    let mut led = Flex::new(pin);
    led.set_as_output(Default::default());
    led.set_high();

    loop {
        led.toggle();
        Timer::after_millis(500).await;
    }
}

#[embassy_executor::main(entry = "hpm_hal::entry")]
async fn main(spawner: Spawner) -> ! {
    let p = hal::init(Default::default());

    spawner.spawn(blink(p.PA23.into())).unwrap();

    loop {
        Timer::after_millis(1000).await;
    }
}
