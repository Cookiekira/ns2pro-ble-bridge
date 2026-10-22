# Linux hardware acceptance (pending)

Record revision, distribution/kernel, architecture, adapter model/firmware,
controller firmware, privileges and whether bluetoothd is running for every run.
The implementation session has not completed this checklist; see
[rust-linux-implementation.md](rust-linux-implementation.md) for exact evidence.

## Prerequisites

- [ ] Root for auto-attach, or CAP_NET_ADMIN for `--no-auto-attach` Bluetooth.
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
- [ ] Host rumble reaches controller and latest-value 20 ms throttle remains stable.
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

- [ ] Capture actual LE connection interval through native events/HCI evidence.
- [ ] Reconcile Linux's ownership-safe preference limitation before claiming standard 7.5 ms request acceptance.
- [ ] Rejection of optimization allows system-parameter connection to continue.
- [ ] Verified 5 ms hardware path: currently unavailable; do not send generic interval 4.
- [ ] Verify controller IMU timestamp semantics or retain monotonic receipt fallback.
- [ ] Record BLE arrival rate/jitter and receipt-to-USB-write p50/p99 separately from interval.
- [ ] Linux x64 hardware acceptance; Linux arm64 native build and hardware acceptance.

Windows delivery has separate build/test evidence; no Windows hardware claim is made.
