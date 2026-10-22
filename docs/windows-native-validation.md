# Windows native implementation evidence

Implementation for [#19](https://github.com/Cookiekira/ns2pro-ble-bridge/issues/19), using the contracts established by #16. Native implementation begins at `73bec84`; shared foundation `44fdbcb` is merged in `c8b7469`; tests and module wiring are in `7600e61`, with adapter-identity lookup deferred in `7551282`.

The Windows modules implement the common Bluetooth/GATT traits and use the common controller, pairing, supervisor and USB/IP server. There is no Windows-specific controller/session protocol. `rt::windows::RuntimeGuard::new()` must be retained on the current-thread runtime thread until shutdown; it initializes the WinRT MTA and owns best-effort MMCSS Games registration. `WindowsBluetooth::new().await` selects the default LE-capable adapter. Only `adapter_address()` reads its identity, so the shared host-address override bypasses that lookup.

The watcher uses active scanning and extended advertisements. Each scan requires a fresh matching advertisement and expires after 30 seconds. Connection uses the discovered address type and uncached service/characteristic discovery; discovery expires after 10 seconds. Dropping an outstanding WinRT operation requests native cancellation. Watcher registrations, notification handlers, service/device handles and connection-preference requests have scoped cleanup. Notifications borrow `IBufferByteAccess` bytes synchronously; the callback does not allocate a report or enqueue an input pump. Connection-status events signal the common cancellation token. ThroughputOptimized is best effort, and logs distinguish its request from the separately observed connection interval (or unknown).

`usb::usbip_win::Attached::attach(SocketAddr, CancellationToken).await` starts a blocking worker for SetupDi discovery and native driver IOCTLs. Bind and serve the shared TCP listener concurrently before awaiting attach: PLUGIN_HARDWARE completes only after the driver performs the USB/IP import. `detach(&mut self).await` is idempotent and waits for worker cleanup. Dropping Attached or cancelling its parent token also requests cleanup. CancelIoEx is followed by completion draining before freeing request storage; a successful import that races cancellation is detached. Each worker owns its device/event handles and detaches only its own positive port. No-auto-attach bypasses Attached and uses the same shared server. The server owns TCP_NODELAY configuration.

The encoder follows upstream released `include/usbip/vhci.h` layouts:

| Driver release ABI | Request size | Busid/service/host offsets | Additional fields |
| --- | ---: | --- | --- |
| v.0.9.6.1 through v.0.9.7.7 | 1100 | 8 / 40 / 72 | None |
| v.0.9.7.8 | 1116 | 8 / 40 / 72 | Empty serial at 1100 |
| v.0.9.8.1 | 1124 | 12 / 44 / 76 | Location hash; empty serial at 1104; low-latency WSK mode at 1120 |

Attach tries released layouts newest first and retries only STATUS_BUFFER_TOO_SMALL / STATUS_INVALID_BUFFER_SIZE, which the checked driver sources return before starting import. Permissions, connection errors, cancellation and other failures never trigger ABI retry. Unknown driver layouts fail with an actionable hint rather than reporting success. Current unreleased upstream master has another layout and is not claimed supported. Structure offsets/sizes were independently checked with clang-cl's MSVC record-layout output. No driver/registry interval patches or usbip executable calls are used.

## Validation performed on Linux, 2026-10-22

Earlier integrated revision checked: `aaff3b6` plus removal of a duplicate Linux `vhci` module declaration introduced by the integration merge, including shared application cleanup `7fad599`, CLI validation fix `13f8093`, lifecycle/latency tests `86a7085`, native pre-cancellation fix `63e04d9`, and native resource-test correction `11d0082`. Application composition `4f28014` was merged with the native smoke method in `9b008e0`.

Host tools: rustc 1.100.0-nightly (0dfb098f3), cargo 1.100.0-nightly, clang-cl/LLD 23.1.3, cargo-xwin 0.23.1, Wine 11.20. The MSVC target was installed; MinGW was absent. cargo-xwin downloaded Microsoft's CRT/SDK for executable linking. Temporary checkers and tooling are outside the repository.

| Check | Result |
| --- | --- |
| `cargo check --target x86_64-pc-windows-msvc --workspace --all-targets` | Passed |
| `cargo clippy --target x86_64-pc-windows-msvc --workspace --all-targets -- -D warnings` | Passed |
| `cargo fmt --all -- --check` | Passed |
| `cargo test --workspace` on Linux | **52 passed**, 2 measurement cases ignored |
| `cargo-xwin build --target x86_64-pc-windows-msvc --workspace --release` | Passed; actual optimized Windows PE executable linked |
| `cargo-xwin test --target x86_64-pc-windows-msvc --workspace --no-run` | Passed earlier; final suite also rebuilt/linked as part of execution below |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test --target x86_64-pc-windows-msvc --workspace` | **49 passed under Wine**, 2 explicitly ignored measurement cases |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test --release --target x86_64-pc-windows-msvc -p ns2pro-bridge --test linux_latency -- --ignored --nocapture --test-threads=1` | Both measurement cases passed; zero measured steady-state allocations |

The 49 tests comprise 13 bridge/native/CLI tests, 3 shared lifecycle tests, 3 shared session tests, 27 protocol tests and 3 fixture tests. Session/lifecycle scenarios exercise actual orchestration and USB/IP bytes through Fake GATT and an in-memory peer: immediate input/output routing, pairing rejection, calibration fallback, pending bulk/UNLINK/STALL/partial headers, cache precedence/invalidation/cancellation, partial setup cleanup, neutral input after disconnect, reconnect LED replay and stale rumble removal.

Native-specific tests exercise real WinRT buffer allocation/IBufferByteAccess pointer identity and initialized length, actual named Windows event-object lifetime, cancellation scope lifetime, failed GATT statuses, pre-import ABI retry classification, pre-cancelled attach, detach completion/idempotence, and five portable IOCTL byte/encoding/error tests. The byte tests also execute in Linux workspace tests. Production OS calls are never replaced with success stubs. The event lifetime test uses a unique kernel-object name because numeric handles may be reused immediately by concurrent test threads.

Release measurement conditions: Windows-target executable under Wine on this Linux host, 10,000 measured samples per case after warmup, one test thread, paused Tokio time and in-memory transport. Pure BLE decode to encoded USB/IP response: p50 100 ns, p99 200 ns, maximum 4,400 ns; Fake GATT notification to in-memory USB/IP peer: p50 4,100 ns, p99 6,000 ns, maximum 21,200 ns. Both reported **0 allocations / 0 allocated bytes** in the measured steady-state region. These figures measure emulated host processing; they do not measure native Windows scheduling, WinRT callback/driver latency, wireless transit or game latency.

`WindowsBluetooth::smoke_test()` enumerates adapters and runs an active extended watcher for 500 ms without requiring a matching controller, then explicitly stops and awaits the stopped event. Drop unregisters/stops on failure/cancellation. Release CLI execution under Wine verified `--help` exits 0 and invalid `--feature-flags` exits 2. `--bt-smoke-test` exits 1 with an actionable Bluetooth error because Wine's adapter API returns E_NOTIMPL; no successful hardware smoke scan is claimed.

Shared app review findings (pending-attach cancellation reporting setup failure; server-first exit dropping rather than draining attach) were fixed centrally in `7fad599` and included in the final build/tests. Native attach additionally checks pre-cancellation before argument encoding or SetupDi discovery, and preserves cancellation during failure cleanup.

Native Windows CI must still build and execute the suite on Windows: Wine is additional evidence, not Windows OS acceptance. No physical Windows Bluetooth adapter, controller, usbip-win2 kernel driver, wireless interval/latency, or virtual-device enumeration acceptance was performed or claimed. Encoding and cleanup tests do not establish driver/hardware operation.

## Rust-only cutover revalidation

Final code revision tested: **`f60f256260a44c3025088f54cf93276980426c01`**, fast-forward merged onto `feat/rust-windows`. This includes Rust-only cutover `b56f3e3`, Windows evidence/fix `b2618c1`, and the controller subscription-race fix plus three additional primary session tests in `f60f256`. No implementation changes were needed during this revalidation; no original-checkout files or deleted legacy sources were edited.

Reran successfully on the same Linux host/toolchain:

- `cargo clippy --target x86_64-pc-windows-msvc --workspace --all-targets -- -D warnings`.
- `cargo fmt --all -- --check` and `git diff --check`.
- `cargo-xwin build --target x86_64-pc-windows-msvc --workspace --release` (actual Windows executable linked).
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test --target x86_64-pc-windows-msvc --workspace`: **52 passed**, 2 measurement tests ignored in this invocation. Counts: 13 bridge/native/CLI, 3 lifecycle, 6 session, 27 protocol, 3 fixtures. All three new discovery/pairing/cache/host-bypass/pair-once, forget/bounded-retry, and leading-edge/latest-rumble/setup-output tests passed.
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test --release --target x86_64-pc-windows-msvc -p ns2pro-bridge --test linux_latency -- --ignored --nocapture --test-threads=1`: **2 passed** explicitly.

Updated release measurements (10,000 samples each, same warmup/single-thread/paused-time/in-memory conditions):

| Measured path under Wine | p50 | p99 | Maximum | Allocations / bytes |
| --- | ---: | ---: | ---: | ---: |
| Pure BLE decode to encoded USB/IP response | 100 ns | 100 ns | 1,700 ns | 0 / 0 |
| Fake GATT notification to in-memory USB/IP peer | 4,700 ns | 6,000 ns | 23,600 ns | 0 / 0 |

These remain cross-build and Wine execution results. Native Windows CI, Bluetooth/controller hardware, usbip-win2 kernel-driver attach, and native Windows latency were not executed or claimed.

## Primary references

- [Microsoft windows crate 0.62.2 generated API source](https://github.com/microsoft/windows-rs/tree/0.62.2/crates/libs/windows/src/Windows).
- [IBufferByteAccess::Buffer](https://learn.microsoft.com/en-us/windows/win32/api/robuffer/nf-robuffer-ibufferbyteaccess-buffer).
- [RequestPreferredConnectionParameters](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothledevice.requestpreferredconnectionparameters).
- usbip-win2 released headers: [v.0.9.7.7](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.7/include/usbip/vhci.h), [v.0.9.7.8](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.8/include/usbip/vhci.h), [v.0.9.8.1](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/include/usbip/vhci.h).
- [Driver validation before import](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/drivers/ude/vhci_ioctl.cpp), [upstream attach/detach implementation](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/userspace/libusbip/src/vhci.cpp).

## Disconnect race final revalidation

The ns2-windows agent fast-forwarded to exact integration code revision
`9f1b0562820b9345f4b4a53b38af553f2026dd41` (including `ef49aca`) and reran strict
Windows target clippy, optimized cargo-xwin workspace linking and Windows-target
tests under Wine. All passed: **53 tests**, 0 failures, 2 opt-in measurements ignored
in the default invocation. Counts: 13 bridge/native/CLI, 3 lifecycle, 7 primary
session, 27 protocol and 3 fixture tests. The late-notification-after-neutral
regression passed. No code edits were needed. These remain cross-build/Wine
results, with native Windows CI/hardware unverified.

## Post-hardware-fix integration revalidation (2026-10-22)

The `feat/rust-windows` worktree was clean before a successful `git merge --ff-only
feat/rust-rewrite`. Exact tested integration revision:
**`be529fc558a3ff53e5f269d14a205253b6156880`**, including Linux readiness fix
`1cc33ff`, documentation update `a04cddb`, and executable/connection logging
`be529fc`. No production code changed during this validation. Only this evidence
file is committed afterward, so its commit can be cherry-picked independently.
The integration worktree and original checkout were not edited.

Tool versions: rustc `1.100.0-nightly (0dfb098f3 2026-08-31)`, cargo
`1.100.0-nightly (e8cb624d5 2026-08-22)`, cargo-xwin `0.23.1` at
`/tmp/ns2-windows-tools/bin/cargo-xwin`, clang-cl/LLD `23.1.3`, Wine `11.20`.
These checks target only `x86_64-pc-windows-msvc` on the Linux host; they do not
establish compatibility with an older Rust compiler or other Windows architectures.

| Executed check | Result |
| --- | --- |
| `cargo clippy --target x86_64-pc-windows-msvc --workspace --all-targets -- -D warnings` | Passed |
| `/tmp/ns2-windows-tools/bin/cargo-xwin build --target x86_64-pc-windows-msvc --workspace --release` | Passed; actual optimized MSVC-target Windows executable linked |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all /tmp/ns2-windows-tools/bin/cargo-xwin test --target x86_64-pc-windows-msvc --workspace` | **53 passed, 0 failed**, 2 opt-in measurements ignored in this invocation |
| `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all /tmp/ns2-windows-tools/bin/cargo-xwin test --release --target x86_64-pc-windows-msvc -p ns2pro-bridge --test linux_latency -- --ignored --nocapture --test-threads=1` | **2 passed, 0 failed**; both previously ignored measurements executed explicitly |
| `cargo fmt --all -- --check`; `git diff --check` | Passed |

The 53 ordinary tests comprise 13 bridge/native/CLI, 3 shared lifecycle, 7 primary
session, 27 protocol and 3 golden fixture tests. This includes the deterministic
late-callback-after-neutral regression. All 55 distinct test cases were therefore
executed across the ordinary suite and explicit measurement invocation.

Release measurements under Wine used 10,000 samples per case, after warmup, one
test thread, paused Tokio time and in-memory transport:

| Measured path | p50 | p99 | Maximum | Allocations / bytes |
| --- | ---: | ---: | ---: | ---: |
| Pure BLE decode to encoded USB/IP response | 100 ns | 100 ns | 100 ns | 0 / 0 |
| Fake GATT notification to in-memory USB/IP peer | 4,300 ns | 8,600 ns | 102,500 ns | 0 / 0 |

These are observed Wine-host timings, subject to its timer resolution and host
scheduling. They do not measure real Windows scheduling, WinRT callback latency,
wireless latency, usbip-win2 driver performance, or game latency.

`file` identifies the linked release artifact as a PE32+ x86-64 console executable.
Its SHA-256 in this build is
`92d047a4fa78b10401ca775dbda27f1d45e8b11ac1be66f3bfd86ed7838fb9f0`.
Under Wine, release `--version` and `--help` both exited 0; version output was
`ns2pro-bridge 0.2.0`. Release `--bt-smoke-test` emitted the new executable-path and
version startup log, then exited 1 with Bluetooth `E_NOTIMPL (0x80004001)` because
Wine does not implement the adapter API used here. A successful Bluetooth scan is
not claimed, and the controller-connection milestone requires real Bluetooth.

**Native Windows CI remains unverified until a remote Windows run.** No physical
Windows Bluetooth/controller tests, usbip-win2 kernel-driver attach, or Windows
hardware acceptance were performed. The Linux hardware fix is present in the
revision but its Linux-specific readiness behavior is not exercised by these
Windows-target tests. No push, main merge, issue closure, publication or extra
agents/workspaces were involved in this validation.

## Split bulk-response fix revalidation (2026-10-22)

Exact tested integration revision: **`d0a0cbe25d206cd27ef2c4ddf0b2af3fc5fa43e3`**,
including shared USB fix `d61bb37`. The Windows worktree was clean before updating.
A fast-forward was initially impossible because its previous evidence commit
`f1ea588` had been cherry-picked as `d0a0cbe`; rebasing onto integration skipped that
already-applied evidence patch and left the branch exactly at `d0a0cbe`, with no
remaining changes. No production code or integration/original checkout was edited.

Using the same installed toolchain (rustc/cargo `1.100.0-nightly`, cargo-xwin
`0.23.1` at `/tmp/ns2-windows-tools/bin/cargo-xwin`, clang-cl/LLD `23.1.3`,
Wine `11.20`), all requested checks passed:

- `cargo clippy --target x86_64-pc-windows-msvc --workspace --all-targets -- -D warnings`.
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all /tmp/ns2-windows-tools/bin/cargo-xwin test --target x86_64-pc-windows-msvc --workspace`: **54 passed, 0 failed**, 2 opt-in measurement tests ignored. Counts: 13 bridge/native/CLI, 3 lifecycle, **8 primary session**, 27 protocol and 3 fixture tests.
- `/tmp/ns2-windows-tools/bin/cargo-xwin build --target x86_64-pc-windows-msvc --workspace --release`: actual optimized MSVC-target Windows executable linked successfully.
- `cargo fmt --all -- --check` and `git diff --check`.

The new real `serve_urbs`/in-memory peer regression,
`flash_reply_survives_split_bulk_reads_without_consuming_the_next_reply`, passed
under Wine. It covers retaining the tail of an 80-byte flash response across
64+16-byte bulk reads, response order, immediate/pending delivery and zero-length
read behavior. The two performance measurements were intentionally not rerun for
this bulk-only fix, as requested; no updated latency claim is made.

These are x86-64 MSVC cross-build and Wine-executed test results on Linux, not a
native Windows CI or hardware run. Native Windows CI, actual usbip-win2 driver
attach, and Bluetooth/controller hardware acceptance remain unverified. Only this
platform evidence file is committed for the coordinator to cherry-pick.
