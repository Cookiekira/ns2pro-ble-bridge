# Linux hardware acceptance (pending)

Record revision, distribution/kernel, architecture, adapter model/firmware,
controller firmware, privileges and whether bluetoothd is running for every run.
User-operated Linux x64 runs now confirm USB enumeration and physical Steam input,
including regular Steam with SteamRT3 disabled. Per-feature items below remain
unchecked unless individually evidenced; see [integration evidence](rust-integration-validation.md).

## Prerequisites

- [x] Root-operated auto-attach/live interval run on the tested setup: [recorded user-operated result](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result) says the user ran the integration release “with sudo”. Non-root capability setup is not verified.
- [x] Usable LE adapter and local vhci attachment on the tested setup: [integration evidence](rust-integration-validation.md#remaining-acceptance-and-limitations) records “Later privileged user runs successfully attached and enumerated the virtual device”; the live result identifies hci0 on the recorded MediaTek adapter.
- [ ] No other active controller session. Bluetoothd is optional, not a dependency.

## Native and USB path

- [ ] Hidden `--bt-smoke-test` enumerates adapters and scans for 500 ms.
- [ ] Discovery identifies Nintendo advertisements and correct LE address type.
- [x] Controller initialization succeeds on the tested setup: [recorded user-operated result](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result) records “controller initialized and receiving input”.
- [ ] Verify first pairing and fresh advertisements for subsequent cached attempts on hardware.
- [ ] AF_UNIX socketpair attaches through vhci sysfs; record exact kernel result.
- [ ] If rejected, connected local TCP-fd fallback attaches; no usbip executable invoked.
- [x] Virtual `057e:2069` enumerates and physical Steam input works: [first hardware run](rust-integration-validation.md#first-hardware-run-and-connect-readiness-regression) records recognition as `057e:2069` and subsequent user confirmation of physical input, including regular Steam with SteamRT3 disabled.
- [ ] Verify Full Speed and both HID/bulk interfaces individually; recheck SDL flash-read warnings with the fixed build.
- [ ] Check all buttons, both calibrated stick ranges and battery/charging values individually.
- [x] User confirms gyroscope/motion control works in the requested game or Steam configuration on the tested Linux setup. Sensor timestamp semantics and quantitative accuracy remain unverified.
- [x] User confirms Steam controller-test Rumble works on the tested Linux setup.
- [ ] Verify sustained/rapid rumble updates and latest-value 20 ms throttle on hardware.
- [x] User confirms player indicators are steady after connection and restore after reconnect on the tested Linux setup. Arbitrary host-selected LED patterns were not separately checked.
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
