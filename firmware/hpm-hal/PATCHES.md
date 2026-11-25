# Local hpm-hal

Copy of [hpm-hal](https://github.com/hpmicro/hpm-hal) at rev `620f58df241d541d5d5cb1575407a16351da49fd`
(MIT/Apache-2.0, see the LICENSE files; `examples/` omitted), used as a path dependency of
`app`. Only the changes below were made, each small enough to go upstream as it is. All of
them were found on the real ZENT75-HE (HPM5361) and are needed for the firmware to work at all.

| File | Bug | Fix |
|---|---|---|
| `src/usb/endpoint.rs`, `src/usb/control_pipe.rs` | ENDPTCOMPLETE is write-1-to-clear. Clearing an endpoint's bit with `modify` wrote back every set bit and so cleared the completions of the other endpoints too; their tasks then waited forever (the vendor interface hung after a few commands). | `write` with only the endpoint's own bit. |
| `src/usb/bus.rs` `endpoint_set_enabled` | SET_CONFIGURATION opened the endpoints without waking tasks parked in `wait_enabled`, so they never sent. | Wake the endpoint's waker. |
| `src/usb/bus.rs` `poll`, `enable` (feature `usb-pin-reuse-hpm5300`) | With reused pins VBUS/session valid are overridden in PHY_CTRL0 and there is no VBUS sense, but `poll` still reported power from OTGSC.ASV. ASV reads 0 at the first poll (the override has not settled) and drops again when `enable` sets OTGSC.VD; embassy-usb answers `PowerRemoved` with `Bus::disable`, which resets controller and PHY, after which ASV never comes back: the device never attaches. | Under the feature, `poll_power` reports `PowerDetected` once and nothing after, and the ASV interrupt stays off. Without the feature the behaviour is unchanged. |
| `src/internal/interrupt.rs` `get_priority`, `set_priority`, `set_priority_with_cs` | The PAC's `PLIC.priority(n)` is the register of source `n + 1` (offset `0x04 + 4·n`), but the IRQ number was passed as `n`: the priority of the *next* source was programmed and the intended one kept its reset value. | Index `number() - 1`. |
| `src/internal/interrupt.rs` `InterruptExt::complete`, `PlicExt::complete` | Completion was a read-modify-write of the claim/complete register. The read *claims* the highest-priority pending source, which is then never serviced or completed, so the PLIC masks every source at or below its priority for good (here: finishing GPTMR1 or USB0 while an ADC0 step was pending stopped the scan). | Plain `write`. This also fixes `bind_interrupts!` and the generated GPIO / time-driver handlers, which complete through it. |

Not a fix: `Cargo.toml` ends with a `[lints.rust]` table that silences upstream's own
`unused` / `unsafe_op_in_unsafe_fn` warnings (as a path dependency the crate is no longer
lint-capped).

To see the exact changes: diff `src/` against the upstream revision.
