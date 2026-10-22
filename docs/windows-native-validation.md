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

Final integrated revision checked: `aaff3b6` plus removal of a duplicate Linux `vhci` module declaration introduced by the integration merge, including shared application cleanup `7fad599`, CLI validation fix `13f8093`, lifecycle/latency tests `86a7085`, native pre-cancellation fix `63e04d9`, and native resource-test correction `11d0082`. Application composition `4f28014` was merged with the native smoke method in `9b008e0`.

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

## Primary references

- [Microsoft windows crate 0.62.2 generated API source](https://github.com/microsoft/windows-rs/tree/0.62.2/crates/libs/windows/src/Windows).
- [IBufferByteAccess::Buffer](https://learn.microsoft.com/en-us/windows/win32/api/robuffer/nf-robuffer-ibufferbyteaccess-buffer).
- [RequestPreferredConnectionParameters](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothledevice.requestpreferredconnectionparameters).
- usbip-win2 released headers: [v.0.9.7.7](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.7/include/usbip/vhci.h), [v.0.9.7.8](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.8/include/usbip/vhci.h), [v.0.9.8.1](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/include/usbip/vhci.h).
- [Driver validation before import](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/drivers/ude/vhci_ioctl.cpp), [upstream attach/detach implementation](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/userspace/libusbip/src/vhci.cpp).
