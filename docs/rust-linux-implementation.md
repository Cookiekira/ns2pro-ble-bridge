# Native Linux Rust implementation and evidence

This records the implementation on `feat/rust-linux` for #17 and the assigned
parts of #18. It is not hardware acceptance of either ticket, and does not claim
that #16 or the final #20 integration is accepted. The original checkout was read
only; implementation and commits are confined to the Linux worktree.

## Implemented surface

- Pure `ns2pro-proto::att` implements MTU exchange, primary-service discovery,
  paged characteristic discovery and notification CCCDs. Characteristic ranges
  include the next declaration across pages and never cross service boundaries.
  Empty services, malformed/nonprogressing responses and missing required
  characteristics fail explicitly. Notification/indication payloads are borrowed.
- Pure `ns2pro-proto::mgmt` encodes management commands, addresses and standard
  7.5 ms preferences, and validates management/EIR framing and controller identity.
  Neither pure module depends on Tokio.
- `ble::linux` selects an LE adapter with READ_INDEX_LIST/READ_INFO and powers it
  when needed. Each scan uses a newly opened HCI control-channel socket and newly
  received DEVICE_FOUND events, including explicit/cached targets. Discovery waits
  at most 30 seconds after START completes. Busy discovery is observed passively;
  only successfully started discovery is stopped. A cancellation during START is
  drained through its completion and owned STOP. `LinuxBluetooth::close().await`
  must run before runtime teardown to drain these workers. Smoke observes for a
  full half second rather than stopping on the first advertisement.
- Raw nonblocking LE L2CAP binds the selected local adapter and fixed CID 4,
  requests BT_SECURITY_LOW and uses the freshly advertised public/random type.
  Connect is limited to 10 seconds. AsyncFd readiness handles EAGAIN and validates
  SO_ERROR. Packet truncation and partial ATT writes fail explicitly.
- ATT discovery/subscription transactions have a two-second deadline. The reader
  invokes notification sinks directly from a reusable buffer, answers peer MTU
  exchange, confirms indications and rejects unsupported requests. EOF/errors
  cancel the link. A cancelled or timed-out untagged transaction closes its bearer
  so a late response cannot satisfy another transaction. Command protocol reply
  validation/serialization is in the shared controller implementation.
- `usb::vhci` selects free `hs` ports from `status*`, attaches a stream socketpair
  by sysfs with Full Speed value 2, and drops the passed fd after successful attach.
  Transport rejection alone permits fallback to a connected local TCP fd; both TCP
  ends use TCP_NODELAY. Busy ports are retried without detaching other ports.
  Explicit detach and Drop cleanup are idempotent. A used port must still match the
  original device/fd before cleanup; during enumeration vhci hides that identity.
  Sysfs has no atomic ownership/lease token, so a different privileged manager
  racing detach/reuse cannot be excluded by userspace checks alone.
- `rt::linux::PriorityGuard` attempts SCHED_FIFO priority 10 and restores the prior
  runtime-thread scheduler on Drop. Failure is debug-only and nonfatal; the guard
  cannot move to another thread.

The bridge uses the shared session, channel and USB/IP stack. It has no D-Bus,
BlueZ subprocess, usbip subprocess, C# interop or Go runtime dependency.

## Live connection interval request

The integration now attempts standard HCI LE Connection Update (0x2013) once per
connection: interval min/max 6/6 (7.5 ms), latency 0, supervision timeout 200
(2000 ms), CE min/max 0. This is non-persistent; LOAD_CONN_PARAM remains unused.
There is no verified PC 5 ms path, and generic interval 4 is never sent.

A raw HCI socket on the selected adapter opens before ATT connect, filtered locally
to LE Meta, Disconnect Complete and command responses. It does not set controller
masks or take HCI_CHANNEL_USER. The connected ATT fd supplies L2CAP_CONNINFO's
native-endian handle; getpeername/getsockname verify both ATT endpoints. Queued
connection events must match the peer/address type, central role and handle.
Disconnect/replacement events invalidate that epoch. The fd is rechecked before
submission and while observing the result. The whole optional operation has a
two-second deadline; errors are logged and GATT setup continues. CAP_NET_RAW is
needed in addition to management's CAP_NET_ADMIN; no capabilities are installed
automatically.

The initial Connection Complete event logs the pre-request interval. A matching
successful LE Connection Update Complete logs the actual observed interval,
latency and supervision timeout separately from the requested 7.5 ms. Status
success alone, unrelated handles, and report rate never count as interval proof.
A different negotiated interval is reported honestly. This observer ends after
startup; later changes by another manager are not tracked.

**Residual concurrency limitation:** userspace validation does not atomically pin
the numeric controller handle across HCI queue execution. A remote disconnect and
reuse in that gap can affect a different connection. This is a best-effort path,
not a strict ownership guarantee. Broadcast command statuses lack requester IDs;
observed updates do not prove exclusive causation. Same-peer L2CAP channels share
the ACL/interval, and closing ATT need not end that ACL. Do not retry or restore a
stale handle. There are no saved-preference changes to undo.

Sources: [BlueZ's standard update helper](https://github.com/bluez/bluez/blob/7bc9684dc9e45358661301da4a0ede433fdf0a30/lib/bluetooth/hci.c#L3069),
[Linux ATT connection-info socket option](https://github.com/torvalds/linux/blob/7be4bc52fd9e0e55dc9a2e0232b89427d2c4b614/net/bluetooth/l2cap_sock.c#L548),
and [Linux live update handling](https://github.com/torvalds/linux/blob/7be4bc52fd9e0e55dc9a2e0232b89427d2c4b614/net/bluetooth/hci_event.c#L6114).

## Why saved preference writes remain disabled

Kernel source reviewed at
[Linux 4403278e4ea2, load_conn_param](https://github.com/torvalds/linux/blob/4403278e4ea2f654f8130a5c03eb8b4516a0f201/net/bluetooth/mgmt.c)
shows that even a single-device LOAD_CONN_PARAM calls
`hci_conn_params_clear_disabled(hdev)` when the target has no existing entry.
That can remove saved entries for unrelated devices. If the target already exists,
it overwrites its stored parameters without returning the previous value. The
[BlueZ management protocol](https://github.com/bluez/bluez/blob/7bc9684dc9e45358661301da4a0ede433fdf0a30/doc/mgmt-protocol.rst)
provides neither a stored-parameter read nor compare-and-restore operation.
GET_CONNECTIONS lists identity, not stored preferences. NEW_CONNECTION_PARAMETER
is a peripheral parameter event, not a snapshot of arbitrary manager writes.
REMOVE_DEVICE is not a safe undo: it changes device/auto-connect state and rejects
entries whose auto-connect mode is disabled/explicit. Monitoring events after
startup cannot reconstruct state already present before startup or provide atomic
cleanup against concurrent managers. No existing-device-safe restore path was
found in these interfaces.

Hardware acceptance of the live 7.5 ms request and a verified 5 ms path remains open.
The previous conclusion that no generic request was possible was too broad: the
standard live HCI update avoids the saved-preference mutation described above.

A separate kernel wire detail found by the real smoke run is handled explicitly:
[hci_mgmt_cmd](https://github.com/torvalds/linux/blob/4403278e4ea2f654f8130a5c03eb8b4516a0f201/net/bluetooth/hci_sock.c)
queues some early Command Status failures and returns zero from `send`, including
Permission Denied. Management sends accept that result and read the queued status;
ATT sends retain strict packet-length checks. Successful Command Status is not
mistaken for Command Complete.

## Automated behavior and measurements

Normal workspace tests include native socket readiness/EOF/truncation, real ATT
wire discovery/subscription/write behavior, peer MTU/indication/error behavior,
transaction timeout, Busy-discovery ownership, fresh target address type,
cancellation during START and successful Status/Complete ordering. Pure tests
cover empty services and declaration boundaries across response pages.

`tests/linux_lifecycle.rs` uses Fake Bluetooth, real shared session orchestration,
a private cache and an in-memory USB/IP peer. It checks explicit-address priority,
cache invalidation versus cancellation, partial setup cleanup, neutral USB button
bytes on disconnect, reconnect LED replay, host-address lookup bypass for an
already selected controller, and stale-rumble removal. The primary complete fake
session tests remain owned by core in `tests/session.rs`.

Release measurements use an isolated test binary with a thread-local counting
System allocator. Counters include allocation/reallocation requests and bytes from
notification/decode entry through response delivery, after preallocation and
warmup. They exclude setup, header submission, result storage and logging.
Both experiments assert zero measured steady-state allocations; timings are
reported, not used as a flaky pass/fail threshold. The async experiment also
asserts alternating button wire values, matching URB sequence and **no advancement
of paused Tokio time** over all measured samples.

Run:

```sh
cargo test --release -p ns2pro-bridge --test linux_latency -- \
  --ignored --nocapture --test-threads=1
```

Reference run: 2026-10-22, revision `86a7085`, AMD Ryzen 7 9700X (8 cores/16
threads), x86_64 Linux `7.3.0-rc6-2-cachyos-rc`, rustc
`1.100.0-nightly (0dfb098f3 2026-08-31)`, LLVM 23.1.0, release profile with LTO and
one codegen unit, no target-cpu override. Tokio current-thread; no real-time
priority, CPU affinity or exclusive machine reservation. Other implementation
agents/builds were active. Each experiment uses 256 warmup iterations followed by
10,000 samples; a fixed 64-byte BLE input and 64-byte USB report/48-byte USB/IP
reply header, one eligible interrupt IN at a time. Tokio time is paused for the
async experiment while elapsed durations use `std::time::Instant`.

| Measured boundary | p50 | p99 | Maximum | Allocations / bytes |
| --- | ---: | ---: | ---: | ---: |
| Pure BLE decode → USB/IP response encoded | 30 ns | 40 ns | 1,390 ns | 0 / 0 |
| Fake GATT sink → in-memory USB/IP peer response | 800 ns | 830 ns | 24,220 ns | 0 / 0 |

These are host-side processing measurements for this reference workload. They
exclude radio transit, real kernel L2CAP, vhci scheduling, USB host processing and
game consumption. They do not establish wireless latency, real hardware acceptance
or performance on other machines. Native ATT's steady-state notification path is
also reviewed to reuse its receive buffer and pass borrowed bytes; the counting
experiment exercises the shared sink-to-USB path, not a live Bluetooth socket.

## Hardware evidence and remaining checks

Read-only inspection and unprivileged smoke on 2026-10-22:

- User uid 1000, effective capabilities `0000000000000000`.
- hci0 enumerated through native READ_INDEX_LIST/READ_INFO, address
  `EC:91:61:88:1C:C4`. USB vendor/product `0e8d:0616`, product `Wireless_Device`.
  Adapter firmware and controller firmware were not verified.
- `cargo run -p ns2pro-bridge -- --bt-smoke-test` returned exit 1 with
  `Bluetooth: start LE discovery: status 0x14` (Permission Denied) and the
  CAP_NET_ADMIN/root hint. Read-only enumeration succeeded; discovery did not start.
- `/sys/devices/platform/vhci_hcd.0` is absent. No driver was loaded/unloaded and
  no capability was installed. No root execution or controller disconnect was
  attempted. No unrelated active controller session was changed.

Still needed, with an available controller and authorized hardware access:

1. CAP_NET_ADMIN/root discovery, bluetoothd coexistence and operation without
   bluetoothd; fresh advertisements and reconnect address type on the real radio.
2. Raw ATT and host pairing/calibration, correct buttons/sticks/IMU/power,
   rumble/LED round trips, controller-loss neutral/reset and reconnect continuity.
3. Load vhci-hcd, validate AF_UNIX attach or actual TCP-fd fallback, enumeration and
   SDL/game initialization, cancellation/detach and fatal kernel-initiated loss.
4. Actual LE interval via connection events/HCI capture; a safe standard preference
   mechanism and rejection-with-continued-input behavior; verified 5 ms hardware
   if such a supported path becomes available.
5. Confirm controller IMU timestamp semantics. Keep shared monotonic notification
   receipt-time fallback until verified; keep the 20 ms rumble default.

No Linux hardware acceptance beyond enumeration is claimed. No Windows hardware
acceptance is claimed. See the existing Linux hardware checklist for the full
interactive acceptance procedure; this record does not check those boxes.

## 2026-10-22 live interval implementation checks

The focused fake-HCI test first failed with the old unavailable-request behavior
(no command reached the peer within its one-second deadline), then passed after
wiring the live request. Six interval tests cover the exact 18-byte HCI packet,
matching actual updates, unrelated handles/statuses, initial legacy/enhanced/v2
connection events, resolved address-type mapping, malformed/truncated packets,
wrong peer/role/type/handle, stale disconnect/reuse, fd query failure/change,
refusal, missing completion, reconnect and negotiated values other than 7.5 ms.
Failure tests keep a separate ATT socket usable. A seventh test checks the native
sockaddr/filter/connection-info ABI sizes. These tests exercise the request with
socketpair peers and injected live-handle snapshots, not a Bluetooth controller.

Validation: locked workspace tests 65 passed, 0 failed, 2 optional measurements
intentionally ignored; Linux and Windows-target workspace/all-target strict
Clippy passed; formatting, diff whitespace checks, Linux optimized release build
and release `--version` passed. No hot-path performance claims are changed.

No privileged hardware update was executed: `sudo -n true` requires a password
in this terminal. The built integration release is ready for a user-operated
restart with the existing test cache. Require a new successful LE update event
showing `observed_interval_ms=7.5` before claiming the radio delay is improved.
Previous 48.75 ms capture remains the baseline, not evidence for this change.
No kernel virtual-HCI handle-reuse test was run; the queue race documented above
is an unresolved UAPI limitation and is not certified away by fake tests.


## 2026-10-22 user-operated 7.5 ms hardware result

After building commit `eb73be3`, the user ran the absolute integration release
with sudo and `/tmp/ns2pro-hardware-cache.json`. Their supplied terminal screenshot
identifies hci0 on the previously recorded MediaTek adapter and shows:

- 09:34:08.516701 UTC: initial LE connection interval 48.75 ms, latency 0,
  supervision timeout 420 ms, before the request.
- 09:34:08.516719 UTC: standard 7.5 ms request submitted for handle 513.
- 09:34:08.863205 UTC: successful LE connection update observed at 7.5 ms,
  latency 0, supervision timeout 2000 ms.
- 09:34:09.547215 UTC: controller initialized and receiving input.

The user then explicitly confirmed that the previously noticeable Steam input
delay disappeared. This establishes the requested interval on this hardware for
this session and the user's perceived improvement. It does not measure complete
button-to-display latency. No new report-rate measurement or independently saved
packet capture was supplied. The earlier 48.75 ms capture remains baseline only.
Long-duration stability, real rejection fallback, other adapters, per-feature output,
Windows hardware and 5 ms support remain separate acceptance items. The documented
numeric-handle queue race remains unchanged. Earlier pending statements above
record the pre-test state and are superseded by this result for this session.


The user subsequently confirmed the requested power-off/reconnect check passed,
including continued 7.5 ms negotiation. This is user-reported hardware acceptance
on the same tested setup. No extra capture, reconnect count or long-duration test
was supplied; it does not establish other-adapter or soak-test coverage.


The user also confirmed Steam controller-test Rumble works. This validates the
basic host-output-to-controller vibration path on the tested setup. Sustained or
rapid updates, measured throttle timing and stale-rumble behavior across reconnect
were not separately exercised by this confirmation.


The user subsequently confirmed player indicators remain steady after connection
and restore after reconnect. This records the observed indicator behavior on the
same Linux setup; arbitrary host-selected LED patterns were not separately tested.


The user also confirmed gyroscope/motion control works in the requested game or
Steam configuration. The specific application/configuration and quantitative
sensor measurements were not supplied. This validates functional motion input
on the tested setup, without establishing sensor timestamp semantics or accuracy.
The user-operated checks now cover working input, negotiated 7.5 ms with perceived
delay removed, power-off/reconnect, basic rumble, steady/restored player indicators
and motion control. This is scoped Linux hardware evidence, not blanket completion
of #18, other platforms, or all items in the hardware checklist.
