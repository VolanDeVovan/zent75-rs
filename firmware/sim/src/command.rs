//! Control line protocol (socket and `--script`).
//!
//! ```text
//! press <key> [mm]        move the key to mm (default: full travel) at the slew rate
//! release <key>|all       back to 0 mm
//! tap <key> [ms]          press fully, release after ms (default 30) of simulated time
//! travel <key> <mm>       like press with an explicit depth
//! state                   active profile, HID state, keys with travel
//! leds                    85 RGB triplets (rrggbb, LED chain order)
//! hold-at-boot <keys…>    hold keys fully pressed, then reboot (power-up with keys held)
//! reboot                  watchdog-style reset: new Keyboard, boot from storage
//! bootloader              like BE FC: device disappears for 2 s, comes back re-flashed
//! sleep <ms>              wait (wall time) before the next command
//! help, quit
//! ```
//! Keys are indices 0..82 or names from `tools/rammap.py` (`A`, `LShift`, `Space`, `#1` for
//! the digit key 1, since a bare number is an index). Lines starting with `#` are comments.

use crate::keynames::parse_key;
use crate::sensor::FULL_TRAVEL_MM;

#[derive(Debug, Clone, PartialEq)]
pub enum Command {
    Press { key: usize, mm: f32 },
    Release { key: Option<usize> },
    Tap { key: usize, ms: u32 },
    State,
    Leds,
    HoldAtBoot(Vec<usize>),
    Reboot,
    Bootloader,
    Sleep(u32),
    Help,
    Quit,
}

pub const HELP: &str = "commands: press <key> [mm] | release <key>|all | tap <key> [ms] | travel <key> <mm> | \
state | leds | hold-at-boot <keys..> | reboot | bootloader | sleep <ms> | help | quit";

fn key(s: Option<&str>) -> Result<usize, String> {
    let s = s.ok_or("missing key")?;
    parse_key(s).ok_or_else(|| format!("unknown key '{s}'"))
}

fn mm(s: &str) -> Result<f32, String> {
    let v: f32 = s.parse().map_err(|_| format!("bad travel '{s}'"))?;
    if !(0.0..=FULL_TRAVEL_MM).contains(&v) {
        return Err(format!("travel {v} outside 0..{FULL_TRAVEL_MM} mm"));
    }
    Ok(v)
}

fn ms(s: &str) -> Result<u32, String> {
    s.parse().map_err(|_| format!("bad milliseconds '{s}'"))
}

/// Parse one line; `Ok(None)` for blank lines and comments.
pub fn parse(line: &str) -> Result<Option<Command>, String> {
    let line = line.trim();
    if line.is_empty() || line.starts_with('#') {
        return Ok(None);
    }
    let mut it = line.split_whitespace();
    let verb = it.next().unwrap().to_ascii_lowercase();
    let args: Vec<&str> = it.collect();
    let max_args = |n: usize| {
        if args.len() > n {
            Err(format!("too many arguments for {verb}"))
        } else {
            Ok(())
        }
    };
    let c = match verb.as_str() {
        "press" => {
            max_args(2)?;
            Command::Press {
                key: key(args.first().copied())?,
                mm: args
                    .get(1)
                    .map(|s| mm(s))
                    .transpose()?
                    .unwrap_or(FULL_TRAVEL_MM),
            }
        }
        "travel" => {
            max_args(2)?;
            Command::Press {
                key: key(args.first().copied())?,
                mm: mm(args.get(1).ok_or("missing mm")?)?,
            }
        }
        "release" => {
            max_args(1)?;
            match args.first() {
                Some(a) if a.eq_ignore_ascii_case("all") => Command::Release { key: None },
                a => Command::Release {
                    key: Some(key(a.copied())?),
                },
            }
        }
        "tap" => {
            max_args(2)?;
            Command::Tap {
                key: key(args.first().copied())?,
                ms: args.get(1).map(|s| ms(s)).transpose()?.unwrap_or(30),
            }
        }
        "state" => {
            max_args(0)?;
            Command::State
        }
        "leds" => {
            max_args(0)?;
            Command::Leds
        }
        "hold-at-boot" => Command::HoldAtBoot(
            args.iter()
                .map(|a| key(Some(a)))
                .collect::<Result<_, _>>()?,
        ),
        "reboot" => {
            max_args(0)?;
            Command::Reboot
        }
        "bootloader" => {
            max_args(0)?;
            Command::Bootloader
        }
        "sleep" => {
            max_args(1)?;
            Command::Sleep(ms(args.first().ok_or("missing ms")?)?)
        }
        "help" => Command::Help,
        "quit" | "exit" => Command::Quit,
        _ => return Err(format!("unknown command '{verb}' ({HELP})")),
    };
    Ok(Some(c))
}
