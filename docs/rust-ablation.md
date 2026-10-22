# Rust abstraction ablation, 2026-10-22

Four cumulative deletion experiments were run on the foundation stack layer,
starting at `f42bbbb`. Each stage retains previously passing deletions and changes
one additional concern. This is a sequential simplification experiment, not a
full-factorial study or statistical proof of a latency improvement. The 7.5 ms
radio request belongs to the upper layer and is not part of this host benchmark.

## Deletions retained

| Stage | Change | Evidence | Decision |
| --- | --- | --- | --- |
| A (`6f2b792`) | Remove `Controller.responses` async mutex; make command/setup/pair/run require exclusive `&mut self`, and make the internal command method private. | All 58 foundation tests pass, including setup, pairing, output replay, throttle, cancellation and reconnect. | Keep: serialization is enforced by ownership, without a runtime lock or a public command interface. |
| B (`7c6c7f2`) | Remove Linux `Shared.writer` async mutex. | All 59 tests pass. A new native socketpair regression passed both before and after removal: two concurrent writers send 512 complete packets through a deliberately full 4096-byte send buffer, preserving packet payloads; cancellation still rejects later writes. | Keep: `SOCK_SEQPACKET` already makes each packet write atomic. The request/reply transaction lock remains. |
| C (`3a25321`) | Move `AttachError` into `usb`; delete the error-only `usb::host` module. | All 59 tests pass with direct imports from both native USB implementations and application composition. Windows-target strict Clippy also checks the Windows callers. | Keep: one fewer module and path segment; error variants and behavior are unchanged. No line-count or speed benefit is claimed. |
| D (`a76ddca`) | Remove unused `backon` and `zerocopy` dependencies and correct the dependency choices in the spec. | Source search found only manifest declarations. All 59 tests pass, including the existing bounded-jitter retry and binary fixture/codec tests. Offline lock regeneration removes five packages without version upgrades. | Keep: direct dependencies -2; lockfile package entries 112 → 107 (`backon`, `fastrand`, `gloo-timers`, `zerocopy`, `zerocopy-derive` removed). |

## Negative control: the transaction lock earns its place

A new `concurrent_requests_wait_for_each_reply` native ATT test starts two CCCD
writes and checks that the second waits for the first untagged reply. It passes
with the existing transaction lock. Temporarily removing the lock makes the test
fail: the peer observes the second request where the first is expected. The lock
was restored, and the ATT tests pass again. The retained test also checks both
successful replies. This is a protocol-serialization requirement, distinct from
atomic packet writes. Removing the extra writer lock does not remove it.

## Host timing and allocations

Each stage explicitly ran the two existing optimized `linux_latency` measurements
with 256 warmup iterations and 10,000 measured samples per case, one test thread,
on this same Linux host/toolchain. These use Fake GATT and an in-memory USB/IP
peer; they do not exercise native ATT output writes, Bluetooth air time, or game
consumption. They establish a steady-state input non-regression check, not the
causal performance benefit of either removed lock. Single-run tail timings are
subject to host scheduling noise; no speedup is claimed from these differences.

| State | Pure decode/encode p99 | Fake GATT → USB/IP p50 / p99 | Allocations / bytes, each case |
| --- | --- | --- | --- |
| Baseline `f42bbbb` | 40 ns | 780 / 820 ns | 0 / 0 |
| A | 40 ns | 780 / 810 ns | 0 / 0 |
| A+B | 40 ns | 790 / 820 ns | 0 / 0 |
| A+B+C | 40 ns | 780 / 830 ns | 0 / 0 |
| A+B+C+D | 40 ns | 790 / 810 ns | 0 / 0 |

Tests and measurements used a separate `/tmp/ns2pro-ablation-target` release
output directory so foundation experiments did not overwrite the user's tested
7.5 ms release executable. Raw local logs are `/tmp/ns2pro-ablation-*-tests.log`,
`/tmp/ns2pro-ablation-*-perf.log` and the negative/control logs. The table records
the results without relying on those temporary files being retained.

```sh
cargo test --locked --workspace
CARGO_TARGET_DIR=/tmp/ns2pro-ablation-target cargo test --locked --release \
  -p ns2pro-bridge --test linux_latency -- \
  --ignored --nocapture --test-threads=1
cargo clippy --locked --workspace --all-targets -- -D warnings
cargo clippy --locked --workspace --all-targets \
  --target x86_64-pc-windows-msvc -- -D warnings
```

## Interfaces retained after design review

These are design-review decisions, not executed deletion experiments:

- `Bluetooth` and `GattLink`: real Linux, Windows and Fake adapters; deleting the
  interfaces would duplicate portable controller/session behavior or move native
  details into those callers.
- Pure protocol/device/URB engine and its reusable effects: the same implementation
  serves native transport, fixture tests and the full in-memory session. Replacing
  it with transport-specific handling would spread protocol knowledge.
- Latest-value watch channels, callback/statistics mutexes and cancellation/lease
  cleanup: they support output coalescing, callbacks from Windows threads, neutral
  input after disconnect and owned native resources. Their presence is not proof
  that every synchronization point can be removed.

No cache format, CLI option, descriptors, fixture bytes, retry policy, 20 ms rumble
throttle, interval request payload or ownership checks were changed. Earlier
physical acceptance remains evidence for the pre-refactor build; these experiments
do not constitute a new physical hardware run.

# Deepening refactor ablation, 2026-10-23

Six deepening refactors were merged first: USB attachment, connection teardown,
statistics events, cache-owned target selection, a shared test fake and shared BLE
timeouts. Each abstraction they introduced was then removed in turn and kept out
only if the workspace tests and strict Linux and Windows-target Clippy still
passed and the removal did not spread complexity to other callers. Where a test
guarded the abstraction, a mutation was used as a negative control.

## Deletions retained

| Change | Evidence | Decision |
| --- | --- | --- |
| Inline `ble::matches_target` and `Characteristic::check_required` back into both BLE adapters and `att::Discovery::finish`. Keep the shared 30 s scan and 10 s connect timeouts. | Both were higher-order wrappers around a one-line address filter and a three-line loop. 91 tests pass; the removed tests only covered the wrappers. The existing ATT discovery tests still cover `finish`. | Keep the deletion. |
| Fold the `Lease` trait into `LeaseAdapter::detach`. | One trait and three single-method impls removed. Removing the detach call fails 3 attachment tests. The attachment tests passed 50/50 repeated runs. | Keep the deletion. |
| Inline `Connection::input_sink` back into `Controller::initialize`. | It was an associated function with no `self` and one caller. | Keep the deletion. |
| Replace the `Connection::report` pass-through with `Connection::established`. | `Connection` already clears the statistics report on disconnect. It now also records the report, and `attempt` no longer reaches into the statistics lock. | Keep the deletion. |

## Negative control: cache selection earns its place

`Selection` (`Cache::select` and `settle`) has one caller and makes `cache` refer
back to `session` errors, so it was a deletion candidate. Removing it and
restoring the inline rule in `attempt` passed every test. However, three
single-condition mutants of the invalidation rule also passed: dropping
`!established`, dropping `!cancelled`, and treating a Bluetooth cancellation as a
failure. With `Selection` and its decision-table test restored, each of these
mutants fails a test. The abstraction is retained for that test surface.

## Known gap

The late-notification regression fails when input is neutralized after the link
closes. It does not fail when `neutral` runs before the `closed` token is
cancelled. That ordering guards a WinRT callback already in flight on another
thread, and the in-process fake cannot deliver a callback between those two steps.
The order is documented in `Connection::disconnect`.

## Retained without an experiment

- `usb::Error` keeps setup and transport I/O failures distinct. Without it, `usb`
  would depend on `app::AppError`.
- The platform-specific error precedence in `run_attached` preserves the
  pre-refactor result when both detach and application drain fail.

Steady-state input after the ablation: Fake GATT → USB/IP p50/p99 890/920 ns and
pure decode/encode p99 40 ns, with 0 allocations in each case. These are
single-run host timings and support a non-regression check only. No hardware run
was made.
