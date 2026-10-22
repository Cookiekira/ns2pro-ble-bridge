# Linux hardware acceptance (pending)

Record revision, distribution/kernel, architecture, adapter model/firmware,
controller firmware, privileges and whether bluetoothd is running for every run.
User-operated Linux x64 runs now confirm USB enumeration and physical Steam input,
including regular Steam with SteamRT3 disabled. Per-feature items below remain
unchecked unless individually evidenced; see [integration evidence](rust-integration-validation.md).

## Prerequisites

- [ ] Root for auto-attach, or CAP_NET_ADMIN for `--no-auto-attach` Bluetooth; add CAP_NET_RAW for the live interval request.
- [ ] A usable LE adapter; `vhci-hcd` loaded for local auto-attach.
- [ ] No other active controller session. Bluetoothd is optional, not a dependency.

## Native and USB path

- [ ] Hidden `--bt-smoke-test` enumerates adapters and scans for 500 ms.
- [ ] Discovery identifies Nintendo advertisements and correct LE address type.
- [ ] Pairing initializes a controller; subsequent cached attempts require a fresh advertisement.
- [ ] AF_UNIX socketpair attaches through vhci sysfs; record exact kernel result.
- [ ] If rejected, connected local TCP-fd fallback attaches; no usbip executable invoked.
- [ ] Full Speed `057e:2069` enumerates, both HID/bulk interfaces work, SDL/game initializes.
- [ ] Buttons, both calibrated sticks, battery/charging and IMU reach host correctly.
- [x] User confirms Steam controller-test Rumble works on the tested Linux setup.
- [ ] Verify sustained/rapid rumble updates and latest-value 20 ms throttle on hardware.
- [ ] Host player LEDs reach controller and replay after reconnect.
- [ ] Loss publishes neutral buttons/sticks/motion, then reconnects after return.
- [ ] Old rumble cannot replay; pending bulk/IN/UNLINK behave correctly.
- [ ] `--forget-device`, explicit address, host override and pair-host-once behave correctly.
- [ ] Failed cache setup invalidates cache; cancellation preserves cache.
- [ ] Ctrl+C during scan/setup/steady use releases controller and detaches owned USB port.
- [ ] Kernel-initiated USB device loss is fatal rather than silently reattaching.
- [ ] Manual/remote TCP attachment works at configured port, busid `1-1`.
- [ ] Run with bluetoothd and separately without it; record coexistence effects.

## Timing and architecture

- [x] Captured Nintendo LE connection event: 48.75 ms, latency 0, supervision 420 ms. Recheck after any connection-parameter change.
- [x] User-operated integration release eb73be3: startup observer reports 48.75 ms before request and a successful LE update to 7.5 ms, latency 0, supervision 2000 ms; controller initialization succeeds. User confirms noticeable Steam input delay disappeared. Evidence is the supplied runtime-log screenshot, not an independently saved HCI packet capture.
- [x] User confirms controller power-off/reconnect works and still negotiates 7.5 ms on the tested adapter. This is user-reported acceptance; no additional trace or soak-test duration was supplied.
- [ ] Save a fresh HCI packet capture for the 7.5 ms run, including outgoing 0x2013 and matching completion.
- [ ] Rejection of optimization allows system-parameter connection to continue.
- [ ] Verified 5 ms hardware path: currently unavailable; do not send generic interval 4.
- [ ] Verify controller IMU timestamp semantics or retain monotonic receipt fallback.
- [x] Recorded about 20.56 reports/s, mean interarrival 48.18 ms, bridge p99 about 24–28 µs; radio/game end-to-end latency is separate.
- [ ] Linux x64 hardware acceptance; Linux arm64 native build and hardware acceptance.

Windows delivery has separate build/test evidence; no Windows hardware claim is made.
