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

Host tools: rustc 1.100.0-nightly (0dfb098f3), cargo 1.100.0-nightly, clang-cl/LLD 23.1.3, cargo-xwin 0.23.1, Wine 11.20. The MSVC target was installed; MinGW was absent. cargo-xwin downloaded Microsoft's CRT/SDK for executable linking. The temporary checker and cross-build tooling are outside the repository.

- `cargo check --target x86_64-pc-windows-msvc --workspace --all-targets`: passed after foundation merge and after `7551282`.
- `cargo test --workspace`: 36 passed on Linux at `7600e61` (6 bridge, 27 protocol, 3 fixture tests).
- `cargo-xwin build --target x86_64-pc-windows-msvc --workspace`: passed; Windows PE executable linked.
- `cargo-xwin test --target x86_64-pc-windows-msvc --workspace --no-run`: passed; all Windows test executables linked.
- `CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_RUNNER=wine WINEDEBUG=-all cargo-xwin test --target x86_64-pc-windows-msvc --workspace`: **42 passed under Wine** at `7600e61` (12 bridge, 27 protocol, 3 fixture tests).
- Isolated Windows-native modules plus the exact shared contracts and protocol source files: `cargo clippy --target x86_64-pc-windows-msvc --all-targets -- -D warnings` passed after `7551282`. Whole-workspace strict clippy at the shared foundation stopped on an existing `pairing.rs` collapsible-match warning, reported to the core owner for integration.

Native-specific tests exercise actual WinRT buffer allocation/IBufferByteAccess pointer identity and initialized length, actual owned Windows event-handle release, cancellation scope lifetime, failed GATT statuses, pre-import ABI retry classification, detach completion/idempotence, and five portable IOCTL byte/encoding/error tests. The latter also execute in Linux workspace tests. They do not replace production OS calls with success stubs.

Shared session harness `5942cad` was merged in `47982a0`, retaining both platform declarations. After that merge:

- Windows-target all-target check and whole-workspace strict clippy both passed.
- Windows-target workspace tests linked and **45 passed under Wine**, including all three shared session scenarios: complete notification-to-USB/output routing, pairing rejection/calibration fallback, and pending bulk/UNLINK/STALL with a partial USB/IP header.
- Linux workspace tests: **44 passed**; formatting check passed after sorting merged module declarations.

Application composition `4f28014` was merged in `9b008e0`, together with `WindowsBluetooth::smoke_test()`: enumerate adapters and run an active extended watcher for 500 ms without requiring a matching controller, then explicitly stop and await the stopped event. Drop unregisters/stops on all failure/cancellation paths. With this entrypoint, Windows release build, strict target clippy and all 45 Wine tests passed again. Release CLI execution under Wine verified `--help` exits 0 and invalid `--feature-flags` exits 2. `--bt-smoke-test` exits 1 with an actionable Bluetooth error because Wine's Bluetooth adapter API returns E_NOTIMPL; no successful hardware smoke scan is claimed.

Shared app review identified cancellation during pending attach producing an erroneous setup exit and a server-first exit dropping attach before draining it. These were reported to the core owner for correction in shared application code. Final integration must rerun the Windows-target suite after shared cleanup/lifecycle changes. Native Windows CI must still build and execute the suite on Windows: Wine is additional evidence, not Windows OS acceptance. No physical Windows Bluetooth adapter, controller, usbip-win2 kernel driver, wireless interval/latency, or virtual-device enumeration acceptance was performed or claimed. Driver ABI encoding and cleanup tests do not prove driver/hardware operation.

## Primary references

- [Microsoft windows crate 0.62.2 generated API source](https://github.com/microsoft/windows-rs/tree/0.62.2/crates/libs/windows/src/Windows).
- [IBufferByteAccess::Buffer](https://learn.microsoft.com/en-us/windows/win32/api/robuffer/nf-robuffer-ibufferbyteaccess-buffer).
- [RequestPreferredConnectionParameters](https://learn.microsoft.com/en-us/uwp/api/windows.devices.bluetooth.bluetoothledevice.requestpreferredconnectionparameters).
- usbip-win2 released headers: [v.0.9.7.7](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.7/include/usbip/vhci.h), [v.0.9.7.8](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.7.8/include/usbip/vhci.h), [v.0.9.8.1](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/include/usbip/vhci.h).
- [Driver validation before import](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/drivers/ude/vhci_ioctl.cpp), [upstream attach/detach implementation](https://github.com/vadimgrn/usbip-win2/blob/v.0.9.8.1/userspace/libusbip/src/vhci.cpp).
