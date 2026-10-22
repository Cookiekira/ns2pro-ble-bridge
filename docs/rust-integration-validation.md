# Rust integration validation — 2026-10-22

The identity flash response at `0x13000` intentionally differs from the retained
VIIPER golden fixture: it returns the physical controller's 64-byte SPI block,
or the neutral `NS2PRO-BRIDGE-00` fallback after a warning-only read failure.
Calibration blocks at `0x13080`/`0x130c0` remain synthetic because input is already
calibrated by the bridge. Linux vhci and Windows usbip-win2 auto-attach wait for
the first identity resolution (including failure); cancellation before resolution
attaches nothing, and reconnects retain the attached device. `--no-auto-attach`
listens immediately and exposes the fallback until identity resolves. Automated
session/gate tests cover this ordering and passthrough; Steam's displayed physical
identity still requires a hardware check.

## First hardware run and connect-readiness regression

The first user-operated privileged run attached vhci and was recognized by Steam
as `057e:2069`, but input remained neutral and the controller LEDs kept scanning.
The captured bridge log showed USB keepalives while BLE receipt rate stayed zero
and controller initialization never wrote the test cache. This establishes USB
enumeration, not a working physical-controller session.

A native socket regression test reproduced an indefinite first-write wait after
nonblocking connection completion: `SO_ERROR == 0` was followed by clearing
Tokio's edge-triggered writable readiness. Preserve that readiness instead.
The test uses real loopback TCP and the same connect-completion/send methods,
requiring neither Bluetooth privileges nor a fake readiness implementation. It
failed before the fix and passed after it. Linux strict clippy, all **57** tests
and the release rebuild passed. The previous performance and Windows results
below remain evidence for their stated revisions, not reruns of this patch.
The user subsequently confirmed physical input works with the corrected integration
executable, including the regular Steam client with SteamRT3 disabled after the
32-bit libusb fix. Individual unobserved hardware checklist items remain pending.

## Physical timing and split USB response follow-up

The user's recorded Bluetooth trace contains a successful LE Enhanced Connection
Complete for the cached Nintendo controller (handle 513, central role): interval
**48.75 ms**, latency **0**, supervision timeout **420 ms**. This is an observed
connection event, not an inference from notification frequency. It establishes
that interval for the captured session, not every later session. Subsequent live
bridge logs show approximately 20.56 BLE reports/s, mean interarrival 48.18 ms,
and receipt-to-completed-USB-write p99 approximately 24–28 microseconds. The
wireless report cadence dominates the perceptible delay; debug logging level does
not turn a release executable into a debug build. The 0.5 ms bridge-only processing
target must not be presented as an end-to-end wireless latency result.

The SDL probes also exposed a distinct USB response fragmentation bug. SDL reads
an 80-byte flash response as separate 64- and 16-byte transfers; the old engine
truncated each response to the first request and discarded its unread tail.
`d61bb37` retains that tail ahead of later replies for both immediate and pending
IN requests, and handles zero-length reads without consuming data. A session-level
USB/IP peer test failed before the fix (the next command's reply replaced the
missing tail) and passed after it. Linux strict clippy, **58 tests**, and release
build passed. The full workspace also passed all 58 tests on the installed Rust
1.95 stable toolchain, in addition to the development nightly. Windows evidence
for this exact code is in the platform record:
**54 tests under Wine**, strict target clippy and MSVC release linking passed.
Real SDL flash-read warnings must be rechecked after restarting into this build;
no real-hardware disappearance of those warnings is claimed yet.

PR #22 subsequently implemented the best-effort standard HCI LE Connection
Update (`0x2013`). The user confirmed 7.5 ms negotiation, disappearance of noticeable
Steam input delay and continued 7.5 ms after power-off/reconnect. See
[Live connection interval request](rust-linux-implementation.md#live-connection-interval-request)
for implementation and the numeric-handle queue race, and
[User-operated 7.5 ms hardware result](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result)
for the scoped hardware evidence. No independent 7.5 ms packet capture is archived;
5 ms and complete end-to-end latency remain unverified.

## Original integrated delivery record

### Legacy Steam client library diagnosis

After the connect-readiness patch, the user confirmed physical input works with
the SteamRT3 client, but wanted the legacy client to work as well. Live inspection
found that the virtual USB node already grants the desktop user read/write ACLs
and the installed Steam udev rules already contain the `057e:2069` USB rule.

The legacy Steam process loads a 32-bit SDL3, while the RT3 path loads a 64-bit
build. Both tested SDL libraries report the same revision
`SDL-release-3.4.0-1576-gab5541ec5`; the driver itself is present in both. With the
legacy process's library search path, the 32-bit probe reports `Couldn't load
libusb` and exposes only generic evdev (4 axes, 21 buttons). The bundled old
32-bit libusb loads but lacks `libusb_interrupt_event_handler`, required by this
SDL revision's libusb loader. At diagnosis time, the system had 64-bit libusb but no lib32-libusb.

A controlled test extracted the configured repository's `lib32-libusb 1.0.30-1`
package into a temporary directory and prepended only its library directory for
the same 32-bit SDL probe. It then selected
`SDL_JOYSTICK_HIDAPI_SWITCH2 (ENABLED)` (6 axes, 15 buttons), matching the 64-bit
probe. USB flash-read timeout warnings were present in both native-driver probes
and are not claimed resolved by this library fix. No Steam libraries or udev
rules were overwritten. The user installed the package and restarted the legacy
Steam client; process maps confirmed `/usr/lib32/libusb-1.0.so.0.6.0` and controller
logs confirmed HIDAPI. Initial retesting still used an older platform-worktree
bridge build. Launching the fixed integration binary restored physical input,
confirmed by the user with SteamRT3 disabled. This distinguishes the two independent
failures: the legacy client dependency and the outdated ATT connection implementation.

The original local integration on `feat/rust-rewrite` began at seed `aba0ad7`.
PR #21 delivered the rewrite and PR #22 the live interval follow-up to main,
through merge commit `62710a2`. Authority remains GitHub issues #15–#20; merging
does not establish full hardware acceptance or release publication.
The protected original checkout at `/home/oddc/Projects/ns2pro-ble-bridge` was used
only for reading reference sources; no cleanup, reset, stash, commit or edits were
performed there. The tracked VIIPER gitlink was removed only from the implementation
worktree with `git rm --cached`; original submodule data and user cache were retained.

## Integrated implementation and provenance

| Work | Commits / evidence |
| --- | --- |
| Shared contracts | `635f552`: Bluetooth/GattLink, borrowed Send notification sinks, typed errors, Discovered identity, attach error, native dependencies/current-thread Tokio |
| Pure ATT/mgmt | Linux contribution `1b1bd9a`; root protocol modules, no Tokio |
| Shared foundation #16 | `44fdbcb`: deterministic AES pairing; complete USB device/descriptors/reports/flash/commands; pure URB engine; controller, cache, supervisor, USB/IP transport/server |
| Session harness and fixes | `5942cad`, `7fad599`, `f60f256`, `ef49aca`: immediate/dirty/keepalive/bulk/UNLINK/STALL/fragmented reads, output, pairing/cache/retry, resource lifetime, cross-thread disconnect guard |
| Native Linux #17/#18 | `fd1856a`, `bef62a5`, `c8135e1`, `86a7085`, `e73baab`; see [Linux record](rust-linux-implementation.md) |
| Native Windows #19 | `73bec84`, `7600e61`, `7551282`, `9b008e0`, `63e04d9`, `11d0082`, `b2618c1`; evidence update `4cd28aa`; see [Windows record](windows-native-validation.md) |
| Composition and cutover #20 | `4f28014`, `7fad599`, `b56f3e3`: native application entrypoint, structured cleanup, Rust-only CI/releases/docs, deletion of C#/Go/VIIPER runtime chain, retained fixtures/GPL attribution |

Linux branch tip `e73baab` and Windows implementation tip `b2618c1` are ancestors
of the integration branch. Windows agent fast-forwarded to integration revision
`9f1b0562820b9345f4b4a53b38af553f2026dd41` for final code revalidation. Subsequent
integration-record and license-line-ending changes do not change that Rust code.

## Automated validation

At the final code revision `9f1b056`:

- `cargo fmt --all --check`: passed.
- `cargo clippy --locked --workspace --all-targets -- -D warnings`: passed on Linux.
- `cargo test --locked --workspace`: **56 passed, 0 failed**, with two opt-in
  measurement cases ignored in the default invocation. Breakdown: 16 bridge/native/
  CLI, 3 lifecycle, 7 primary session, 27 protocol and 3 golden fixture tests.
- `cargo build --locked --release -p ns2pro-bridge`: passed; stripped Linux x64
  executable linked. Release profile uses LTO, one codegen unit, abort-on-panic and
  portable optimization; no `target-cpu=native` setting.
- `cargo tree -p ns2pro-proto -p ns2pro-usb --edges normal`: neither pure crate
  depends on Tokio, Bluetooth platform code or an application runtime.
- Windows `cargo clippy --target x86_64-pc-windows-msvc --workspace --all-targets --
  -D warnings`: passed, performed by ns2-windows on the same code revision.
- `cargo-xwin build --target x86_64-pc-windows-msvc --workspace --release`: passed;
  actual optimized MSVC PE executable linked with Microsoft's CRT/SDK.
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test
  --target x86_64-pc-windows-msvc --workspace`: **53 passed, 0 failed**, executed
  under Wine 11.20. Two opt-in measurements were separately executed and passed.
  Windows has 13 bridge/native/CLI tests, versus Linux's 16; shared tests are equal.
- CLI release checks on Linux: help 0, invalid feature flags 2, malformed USB address
  2, case-insensitive `--log-level Debug` accepted. `RUST_LOG` has composition-level
  precedence. Retired `--verify-native-library` is rejected by a CLI test.
- `git diff --check aba0ad7..HEAD`: passed after normalizing the retained GPL text
  to LF (license text unchanged).

Primary tests use real orchestration, protocol and USB/IP/device logic with Fake
Bluetooth/GATT and an in-memory byte peer. They cover discovery pairing/cache,
host override bypass, pair-host once across reconnects, initialization/calibration
fallback/rejection, buttons/sticks/IMU/power, host LED/rumble, pending IN, dirty state,
4 ms idle keepalive with unchanged timestamps, pending bulk, UNLINK found/missing,
STALL, fragmented reads, neutral disconnect, late callback rejection, LED replay,
stale-rumble removal, cache precedence/invalidation/cancellation, forget, bounded
cancellable retries and cleanup. Pure tests exercise deterministic AES, malformed
reports, command/address/advertisement codecs, ATT service boundaries/empty services,
mgmt/EIR framing, and all exported descriptor/control/scenario/management fixtures.

Native tests use real socket/event/buffer primitives without Bluetooth or USB
hardware: packet boundaries/EOF, full ATT discovery/subscription/write sequence,
MTU/indication/error handling, timed-out untagged transaction closure, mgmt Busy
ownership and cancellation during START, vhci parsing/fallback classification,
IBufferByteAccess borrowing, Windows event lifetime and attach cancellation/IOCTL
layouts. There are no success-returning production OS stubs.

## Performance measurement

Run explicitly, independent of normal tests:

```sh
cargo test --locked --release -p ns2pro-bridge --test linux_latency -- --ignored --nocapture --test-threads=1
```

10,000 measured samples after 256 warmups, fixed reply buffers, current-thread Tokio,
paused logical time, one test thread, no affinity or priority elevation. Host: AMD
Ryzen 7 9700X, x86_64 CachyOS kernel `7.3.0-rc6-2-cachyos-rc`; rustc
`1.100.0-nightly (0dfb098f3 2026-08-31)`. The tests assert no logical timer advancement
and zero allocations in their measured steady-state input region. They report
rather than enforce a flaky wall-clock p99 threshold. The measured path excludes
real kernel/vhci, radio transit, controller firmware and host game consumption.

Final local rerun on the final Rust code revision:

| Measured Linux path | p50 | p99 | Maximum | Allocations / bytes |
| --- | ---: | ---: | ---: | ---: |
| Pure BLE decode to encoded USB/IP response | 30 ns | 40 ns | 41 ns | 0 / 0 |
| Fake GATT callback to in-memory USB/IP peer | 770 ns | 801 ns | 21,030 ns | 0 / 0 |

Both opt-in tests passed explicitly. Earlier independent Linux and Wine results
are retained in the platform records. Both paths are below the 0.5 ms host-processing
target in this synthetic reference environment. This is not hardware latency acceptance.

## Remaining acceptance and limitations

| Criterion | Actual status / required next verification |
| --- | --- |
| Original unprivileged Linux scan | hci0 enumerated (`EC:91:61:88:1C:C4`, USB `0e8d:0616` Wireless_Device); START_DISCOVERY rejected status `0x14` Permission Denied, exit 1; uid 1000, effective capabilities 0. No successful scan/controller connection claimed. |
| vhci auto-attach / fallback | [v0.2.0 acceptance](linux-hardware-checklist.md#2026-10-22-v020-acceptance-run): the kernel accepted the AF_UNIX socketpair (fallback not needed; unit-tested only). Ctrl+C frees the port, and kernel-initiated detach makes the bridge exit without reattaching. |
| Controller/USB hardware | User confirmed working physical input in SteamRT3 and regular 32-bit Steam after both fixes; cached controller and virtual USB enumeration are observed. Later user confirmations cover functional motion, basic rumble, steady/restored player indicators and power-off/reconnect; see [user-operated result](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result). The [v0.2.0 acceptance run](linux-hardware-checklist.md#2026-10-22-v020-acceptance-run) adds per-item input, battery/charging, LED, rumble, disconnect/reconnect, cache invalidation, remote attach, bluetoothd coexistence (running and masked) and adapter firmware. Controller firmware is unavailable on Linux. |
| Standard 7.5 ms preference | PR #22 implements a best-effort live HCI update without saved-preference writes; see [Live connection interval request](rust-linux-implementation.md#live-connection-interval-request). Startup observed 7.5 ms, latency 0, supervision 2000 ms; user confirmed delay disappeared and power-off/reconnect retained 7.5 ms. See [user-operated result](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result) and [checklist timing evidence](linux-hardware-checklist.md#timing-and-architecture). The [v0.2.0 acceptance run](linux-hardware-checklist.md#2026-10-22-v020-acceptance-run) records an HCI capture of the outgoing 0x2013 command and its matching completion, plus continued input when the request cannot run (`CAP_NET_RAW` dropped). The numeric-handle queue race remains a UAPI limitation; a 30-minute `v0.2.0` soak remains to be recorded. |
| Verified 5 ms / actual interval | No verified 5 ms path; no generic interval 4/vendor command sent. The earlier 48.75 ms / approximately 20.56 Hz session is the baseline; the later startup LE update confirms 7.5 ms on the tested adapter. True end-to-end game latency remains unmeasured. |
| Motion timestamp semantics | Controller timestamp offset not verified; implementation retains monotonic notification-receipt microseconds. Keepalive preserves that timestamp. |
| Windows native CI / hardware | MSVC cross-build plus tests executed under Wine passed. Native CI run 39772269154 at `d951bb1` passed the `windows` job (build, strict clippy, 56 tests). Physical WinRT/usbip-win2 kernel-driver and timing acceptance not claimed. |
| Linux arm64 | Native ARM CI build/strict clippy/tests passed in the `linux-arm64` job of run 39772269154; release dry run 39769150686 built the linux-arm64 artifact. ARM hardware acceptance remains unverified. |
| Publication | PR #21/#22 merged to main through `62710a2`, including the live request and [recorded user results](rust-linux-implementation.md#2026-10-22-user-operated-75-ms-hardware-result). Release dry run 39769150686 built all three artifacts; release run 39772680571 published tag `v0.2.0` with `ns2pro-ble-bridge-v0.2.0-{linux-x64,linux-arm64,win-x64.exe}`. Issue acceptance remains separate. |

## Open acceptance by issue

Updated after the [v0.2.0 acceptance run](linux-hardware-checklist.md#2026-10-22-v020-acceptance-run).

| Issue | User-operated Linux hardware checks still open | Windows physical / arm64 hardware | Remaining software / delivery gaps |
| --- | --- | --- | --- |
| #17 native Linux | None: the native/USB checklist is complete for Linux x64. | No arm64 hardware available; arm64 is CI-only. | Diagnostics follow-ups only (CAP_NET_RAW warning, vhci-loss message, `localhost` note, pairing log). |
| #18 timing | None. A ~24-minute soak with 0 disconnects is recorded in the checklist. HCI capture, failure fallback, bluetoothd coexistence and cache/reconnect are recorded; IMU timestamps keep the monotonic fallback; rumble stays at 20 ms; 5 ms has no verified path. | Windows interval/latency and arm64 timing remain unverified. | Debug statistics observed-interval reporting merged in PR #24. Numeric-handle queue race remains a UAPI limitation; no verified 5 ms implementation. |
| #19 native Windows | — | Post-release hardware validation, outside #19's software acceptance: physical WinRT BLE, usbip-win2 attach/enumeration, input/output, reconnect, cleanup and timing checks remain open; see [Windows record](windows-native-validation.md). | None established: native Windows CI passed in run 39772269154; cross-build/Wine evidence is also recorded. |
| #20 integration/release | None for x64: final acceptance recorded with the published binary (sha256 recorded). | Windows physical and Linux arm64 hardware acceptance remain open. | None established: native CI passed on all three platforms (run 39772269154), release dry run 39769150686 built all three artifacts, and release run 39772680571 published `v0.2.0`. |
