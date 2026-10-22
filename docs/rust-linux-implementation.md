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

## Connection preference limitation

Target: 7.5 ms (standard interval min/max 6/6, latency 0, supervision timeout 200).
**No preference is sent on the current generic path.** It logs the unavailable
optimization and continues with system-allowed parameters. Observed interval is
reported as `unknown`; report arrival frequency is never used as interval proof.
No PC adapter/firmware-specific 5 ms mechanism has been verified here. In
particular, generic interval 4 is never sent.

This limitation is deliberate and was agreed during implementation: do not
mutate unknown shared kernel preferences destructively. The codec and byte test
for a one-device standard request remain available for a future ownership-safe
mechanism.

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

The preference/verified-5-ms hardware acceptance items therefore remain open.

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
