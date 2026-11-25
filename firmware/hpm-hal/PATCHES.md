# Local hpm-hal

Copy of [hpm-hal](https://github.com/hpmicro/hpm-hal) at rev `620f58df241d541d5d5cb1575407a16351da49fd`
(MIT/Apache-2.0, see the LICENSE files; `examples/` omitted), used as a path dependency of
`app`. Only the changes below were made, each small enough to go upstream as it is. All of
them come from running on the real ZENT75-HE (HPM5361).

### Needed to run at all

| File | Bug | Fix |
|---|---|---|
| `src/usb/endpoint.rs`, `src/usb/control_pipe.rs` | ENDPTCOMPLETE is write-1-to-clear. Clearing an endpoint's bit with `modify` wrote back every set bit and so cleared the completions of the other endpoints too; their tasks then waited forever (the vendor interface hung after a few commands). | `write` with only the endpoint's own bit. |
| `src/usb/bus.rs` `endpoint_set_enabled` | SET_CONFIGURATION opened the endpoints without waking tasks parked in `wait_enabled`, so they never sent. | Wake the endpoint's waker. |
| `src/usb/bus.rs` `poll`, `enable` (feature `usb-pin-reuse-hpm5300`) | With reused pins VBUS/session valid are overridden in PHY_CTRL0 and there is no VBUS sense, but `poll` still reported power from OTGSC.ASV. ASV reads 0 at the first poll (the override has not settled) and drops again when `enable` sets OTGSC.VD; embassy-usb answers `PowerRemoved` with `Bus::disable`, which resets controller and PHY, after which ASV never comes back: the device never attaches. | Under the feature, `poll_power` reports `PowerDetected` once and nothing after, and the ASV interrupt stays off. Without the feature the behaviour is unchanged. |
| `src/internal/interrupt.rs` `get_priority`, `set_priority`, `set_priority_with_cs` | The PAC's `PLIC.priority(n)` is the register of source `n + 1` (offset `0x04 + 4·n`), but the IRQ number was passed as `n`: the priority of the *next* source was programmed and the intended one kept its reset value. | Index `number() - 1`. |
| `src/internal/interrupt.rs` `InterruptExt::complete`, `PlicExt::complete` | Completion was a read-modify-write of the claim/complete register. The read *claims* the highest-priority pending source, which is then never serviced or completed, so the PLIC masks every source at or below its priority for good (here: finishing GPTMR1 or USB0 while an ADC0 step was pending stopped the scan). | Plain `write`. This also fixes `bind_interrupts!` and the generated GPIO / time-driver handlers, which complete through it. |

### USB robustness against what hosts do around a reboot

Found after the keyboard ended up in the UF2 loader when the host PC rebooted (a watchdog
reset: something kept thread mode from running for 2 s), and reproduced with the scripted host
of the emulator (`emu/usbhost.py`, `emu/run.py --usb-host reboot` / `vbus-glitch`):

| File | Bug | Fix |
|---|---|---|
| `src/usb/mod.rs` `on_interrupt` | The handler returned early when no enabled USBSTS bit was set, *before* looking at OTGSC. An A-session-valid change (ASVIS, enabled by `Bus::enable`) that arrives without USBSTS activity was therefore never cleared: the interrupt re-entered forever and starved every lower priority and thread mode. | OTGSC is handled before the early return. (With `usb-pin-reuse-hpm5300` the ASV interrupt is no longer enabled at all, see above.) |
| `src/usb/endpoint.rs` `transfer` (ep0) | `while ENDPTSETUPSTAT & 1 {}` before priming a control stage — taken from TinyUSB, where the interrupt handler clears the flag. Here only `ControlPipe::setup` clears it, in the same task: when the host abandons a control transfer and sends a new SETUP before the next stage is primed, the task spins forever. | No wait: `Err(TransferError::SetupPending)`, the stage is given up. |
| `src/usb/control_pipe.rs` `data_in`, `data_out`, `accept` | The waits for a stage to complete had no way out. After a bus reset or a new SETUP in the middle of a control transfer the control task waited forever: no bus events and no SETUP were served any more (the device was dead on the bus until a power cycle). | `wait_stage` also ends on a pending SETUP or a flagged bus reset, flushes ep0 and returns `Disabled`; `setup` flushes ep0 when it takes a SETUP, so a stage the host never collected cannot answer the next request; the interrupt handler wakes both ep0 wakers on SETUP and on reset. |
| `src/usb/bus.rs` `poll` | After the first bus reset the reset interrupt was switched *off* again (`set_ure(false)`), so no later reset (the second one of every enumeration, every re-enumeration by firmware, boot loader and OS) reached embassy-usb: endpoints kept transfers of the previous session, tasks waited on completions that could not come. | `set_ure(true)`. |
| `src/usb/mod.rs` `on_interrupt`, `src/usb/control_pipe.rs` `setup` | USBSTS is write-1-to-clear; `modify` cleared every pending bit, also those not handled (a bus reset that arrived while its interrupt was masked was lost). | The handler clears exactly the bits it handles, `setup` only `UI`. |
| `src/usb/endpoint.rs` `read`, `write` | An endpoint closed during a transfer returned `Ok` (with whatever the queue head said, for `read`), and the wait for the zero-length packet after a full-size IN packet had no exit at all. | `Err(Disabled)`; the ZLP wait ends when the endpoint is closed. |
| `src/usb/bus.rs` `endpoint_close`, `device_bus_reset`, `src/usb/control_pipe.rs` `setup` | Unbounded register polls in thread mode (`while … {}`); the flush poll in `endpoint_close` also compared a bit mask with `== 1`, which only ever matched endpoint 0. | `spin_until` bounds every poll (`SPIN_LIMIT`), the flush is retried at most `FLUSH_TRIES` times, the mask test is `!= 0`. |

Still unbounded upstream and unused here: `remote_wakeup` (`while fpr {}`), the PHY/controller
reset polls in `device_init` (run once, at attach).

Not a fix: `Cargo.toml` ends with a `[lints.rust]` table that silences upstream's own
`unused` / `unsafe_op_in_unsafe_fn` warnings (as a path dependency the crate is no longer
lint-capped).

To see the exact changes: diff `src/` against the upstream revision.
