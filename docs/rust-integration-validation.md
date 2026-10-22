# Rust integration validation — 2026-10-22

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
Physical BLE initialization/input must be rechecked after restarting the old
running executable; hardware acceptance is still pending.

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
SDL revision's libusb loader. The system has 64-bit libusb but no lib32-libusb.

A controlled test extracted the configured repository's `lib32-libusb 1.0.30-1`
package into a temporary directory and prepended only its library directory for
the same 32-bit SDL probe. It then selected
`SDL_JOYSTICK_HIDAPI_SWITCH2 (ENABLED)` (6 axes, 15 buttons), matching the 64-bit
probe. USB flash-read timeout warnings were present in both native-driver probes
and are not claimed resolved by this library fix. No Steam libraries or udev
rules were overwritten. Installing the system package and restarting the full
legacy Steam client remains an interactive verification step; this proves the
library-loading cause and driver selection, not complete Steam gameplay acceptance.

The Rust replacement is integrated locally on `feat/rust-rewrite`, beginning at
seed `aba0ad7`. Authority remains GitHub issues #15–#20. This record does not mark
hardware acceptance complete, close issues, publish artifacts or merge main.
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
| Linux native scan | hci0 enumerated (`EC:91:61:88:1C:C4`, USB `0e8d:0616` Wireless_Device); START_DISCOVERY rejected status `0x14` Permission Denied, exit 1; uid 1000, effective capabilities 0. No successful scan/controller connection claimed. |
| vhci auto-attach / fallback | `/sys/devices/platform/vhci_hcd.0` unavailable. AF_UNIX acceptance, TCP-fd fallback on kernel rejection, detach and fatal host loss require privileged hardware runs. |
| Controller/USB hardware | Pairing, enumeration/SDL, physical input/IMU/power/rumble/LEDs/reconnect and bluetoothd coexistence remain unverified; adapter/controller firmware unknown. Follow [checklist](linux-hardware-checklist.md). |
| Standard 7.5 ms preference | **Unmet software/platform criterion.** Codec encodes 6/6, latency 0, timeout 200, but no production request is sent: examined mgmt LOAD_CONN_PARAM may clear other devices' disabled preferences and provides no read/compare/restore API. Backend logs optimization unavailable and connects using system-allowed parameters. An ownership-safe mechanism must be established before claiming #18 completion. Kernel evidence is in the Linux record. |
| Verified 5 ms / actual interval | No verified adapter/firmware path. No invalid generic interval 4 or unverified vendor command sent. Observed Linux interval and real wireless latency unknown. |
| Motion timestamp semantics | Controller timestamp offset not verified; implementation retains monotonic notification-receipt microseconds. Keepalive preserves that timestamp. |
| Windows native CI / hardware | MSVC cross-build plus tests executed under Wine passed. Actual Windows hosted CI has not run locally; physical WinRT/usbip-win2 kernel-driver and timing acceptance not claimed. |
| Linux arm64 | Native ARM release runner configured, but no ARM build/hardware acceptance performed in this x64 session. |
| Publication | Rust-only workflows prepared; no workflow run, push, tag, release, main merge or issue closure performed. |

#17, #18 and #20 must not be presented as fully accepted while the above hardware
and Linux preference items remain open. The local implementation and all feasible
automated validation are delivered; the parent specification was not modified.
