# Local hpm-hal

Copy of [hpm-hal](https://github.com/hpmicro/hpm-hal) at rev `620f58df241d541d5d5cb1575407a16351da49fd`
(MIT/Apache-2.0, see the LICENSE files; `examples/` omitted), used through `[patch]` in
`app/Cargo.toml`. Changes, all found on the real ZENT75-HE (bring-up probes, see
`docs/firmware-plan.md`):

- `usb/endpoint.rs`, `usb/control_pipe.rs`: ENDPTCOMPLETE is write-1-to-clear. Clearing an
  endpoint's bit with `modify` wrote back every set bit and so cleared the completions of the
  other endpoints too; their tasks then waited forever (the vendor interface hung after a few
  commands). Now `write` with only the own bit.
- `usb/bus.rs`: `endpoint_set_enabled` wakes the endpoint's waker. Before, SET_CONFIGURATION
  opened the endpoints without waking tasks parked in `wait_enabled`, so they never sent.

Still worked around in the app instead (see `app/src/plic.rs`, `app/src/usb.rs`): PLIC
`set_priority` off by one, `complete()` claiming via read-modify-write, and VBUS/ASV power
events (`AlwaysPowered`).
