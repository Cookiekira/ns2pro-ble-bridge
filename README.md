# NS2Pro BLE Bridge

A single native Rust executable bridges a Switch 2 Pro Controller over BLE to a
complete virtual USB NS2Pro (Full Speed, Nintendo `057e:2069`). Version 0.2.0 uses
native Linux Bluetooth/ATT and vhci, or Windows WinRT Bluetooth and usbip-win2.

Build with a current stable Rust toolchain (edition 2024):

```sh
cargo build --locked --release -p ns2pro-bridge
cargo test --locked --workspace
```

The executable is `target/release/ns2pro-ble-bridge` (`.exe` on Windows). No .NET,
Go, D-Bus library, submodule initialization or separate VIIPER process is needed.
The release workflow is configured to build Linux x64, Linux arm64 **natively on
an ARM runner**, and Windows x64, named `ns2pro-ble-bridge-<tag>-<rid>` (Windows adds `.exe`).
Manual dispatch builds and uploads artifacts without publishing a GitHub release;
only `v*` tag pushes publish releases. CI also checks native Linux arm64 builds,
strict clippy and tests. CI run 39772269154 passed the Linux x64, Linux arm64 and
Windows jobs; release dry run 39769150686 built all three artifacts, and release run
39772680571 published `v0.2.0`. Linux arm64 hardware remains unverified.

## Linux

A Bluetooth LE adapter and the `vhci-hcd` kernel module are required. Normal local
attachment uses the kernel sysfs interface, with a stream socketpair and connected
TCP-fd fallback if the kernel rejects AF_UNIX. It does not invoke `usbip` or BlueZ
utilities. Bluetooth uses HCI mgmt and fixed-CID ATT, without SMP pairing or D-Bus.

```sh
sudo modprobe vhci-hcd
sudo ./target/release/ns2pro-ble-bridge
```

Put the controller into pairing mode on first use. The bridge pairs the host,
saves the address, and on later attempts waits for a fresh advertisement before
connecting. `bluetoothd` may run alongside the bridge; actual coexistence still
needs the hardware checks below.

For a non-root manual server, grant Bluetooth management permission to the built
executable (reapply after replacing it). CAP_NET_RAW enables the optional live
7.5 ms request; with only CAP_NET_ADMIN, Bluetooth still works at system parameters:

```sh
sudo setcap cap_net_admin,cap_net_raw+ep ./target/release/ns2pro-ble-bridge
./target/release/ns2pro-ble-bridge --no-auto-attach --usb-addr 0.0.0.0:3241
```

Manual/remote clients connect to USB/IP port 3241, busid `1-1`. A client using the
Linux `usbip` utility can use `usbip --tcp-port 3241 list -r HOST` and
`sudo usbip --tcp-port 3241 attach -r HOST -b 1-1`; that utility is only required on
such a client. The server is unauthenticated: bind/expose it only to trusted peers.
Permission errors need root/CAP_NET_ADMIN; attach errors need root and `vhci-hcd`.
A kernel-initiated virtual-device loss ends the application, while controller loss
neutralizes input and reconnects. Ctrl+C closes the controller and detaches USB.

## Windows

Use a Bluetooth LE adapter and install the signed
[usbip-win2](https://github.com/vadimgrn/usbip-win2) driver using its installation
instructions, then run `ns2pro-ble-bridge.exe`. Default mode serves USB/IP and
attaches through native SetupDi/IOCTL on a blocking worker. Supported released
request layouts cover v0.9.6.1–v0.9.7.7, v0.9.7.8 and v0.9.8.1. Arbitrary future
ABIs are not assumed compatible. Driver/access errors include recovery hints.
`--no-auto-attach` serves the same TCP protocol without driver attachment.

The executable is cross-linked for Windows MSVC and automated tests have run under
Wine. Native Windows CI builds, runs strict clippy and passes the tests on
`windows-latest` (run 39772269154). No Windows Bluetooth/driver hardware
acceptance is claimed. See [Windows evidence](docs/windows-native-validation.md).

## Options

| Option | Default / behavior |
| --- | --- |
| `--usb-addr HOST:PORT` | `localhost:3241`; used for manual Linux and Windows serving |
| `--device-address MAC` | Explicit address overrides cache and discovery selection |
| `--pair-host` | Pair again, once successfully per process across reconnects |
| `--host-address MAC` | Pairing host identity override; bypasses adapter identity lookup |
| `--forget-device` | Remove cached selection before reconnect supervision |
| `--cache-file PATH` | Local app data / `Ns2Pro.BleBridge/controller-cache.json` |
| `--no-auto-attach` | Serve TCP USB/IP for manual/remote clients |
| `--feature-flags FLAGS` | `0x07` (buttons, sticks, IMU); hexadecimal or decimal |
| `--log-level LEVEL` | `info`; `RUST_LOG` takes precedence |
| `-h`, `--help` | Show help |

The cache remains JSON `{"address":"AA:BB:CC:DD:EE:FF"}`. Failed cached setup
invalidates it; cancellation preserves it. Retries use cancellable 1–10 second
exponential backoff with jitter; established-session loss waits one second.
Disconnect clears buttons/motion, preserves host LEDs (default `0x06`), and drops
old rumble. Rumble sends immediately when eligible and then only the latest value
at least 20 ms apart. Exit codes: 0 normal shutdown, 1 runtime failure, 2 invalid
parameters or USB setup failure. The hidden `--bt-smoke-test` enumerates adapters
and scans for half a second. Retired native-library diagnostics are removed.

## Timing and verification

### Steam on Linux: detected controller but no input

The legacy 32-bit Steam client also needs a sufficiently recent **32-bit libusb**
for SDL's Switch 2 driver. On Arch/CachyOS, install it with:

```sh
sudo pacman -S --needed lib32-libusb
```

Then fully exit and restart Steam. The machine can already have the Nintendo
`057e:2069` USB access rule and 64-bit libusb while this dependency is still
missing. In the observed failure, Steam's bundled old 32-bit libusb loaded but
lacked `libusb_interrupt_event_handler`, so SDL logged `Couldn't load libusb`
and fell back to generic evdev instead of its Switch 2 HIDAPI driver. Running
that same 32-bit SDL with the current libusb selected the Switch 2 driver.
The experimental SteamRT3 client used a working 64-bit library path, which hid
the legacy client's dependency issue; enabling RT3 is not the required fix for
this particular library-loading failure. Device permissions and other client
failures must still be diagnosed independently.

### Input timing

Input notifications decode directly into latest state and wake pending USB IN
requests; idle reports use 4 ms keepalives with unchanged sample timestamps. The
bridge uses monotonic receipt microseconds for IMU timestamps until controller
timestamp semantics are verified. Debug logs every five seconds report BLE rate,
interarrival variation, receipt-to-USB-write p50/p99, input URB rate, keepalives and
rumble writes. These measurements exclude wireless transit and host consumption.

The target is 5 ms BLE only on a verified adapter/firmware-specific path; **no such
path is implemented or verified**. Linux now makes one best-effort standard
7.5 ms request on the connected ATT channel's HCI handle, without changing saved
connection preferences. A socket-local observer starts before connection and
checks peer, address type, central role, handle and disconnect/reuse events. The
request needs CAP_NET_RAW (already available when run as root); unavailable
permissions, refusal or a two-second observation timeout continue normal setup.
Logs distinguish `requested_interval_ms` from `observed_interval_ms`; only a
matching successful LE update event confirms the observed value. Command Status
and report rate do not prove the actual interval.

Linux exposes no atomic fd-bound HCI update: a disconnect/handle reuse between
validation and queued command execution remains possible despite these checks.
Other channels on the same peer share this ACL and its interval; other managers
may subsequently change it. There is one attempt per connection, no retry fight,
no stale rollback, and no monitoring claim after startup. Actual hardware acceptance
of 7.5 ms is confirmed on the tested Linux adapter by the startup LE update
event and user confirmation that noticeable Steam input delay disappeared.
The user also confirmed power-off/reconnect retains 7.5 ms. Long-duration stability
and rejection on real hardware remain pending. Windows
requests ThroughputOptimized best-effort.

Tests compare all exported USB fixture scenarios and supported descriptors, with
explicit STALL/pending bulk/UNLINK differences. Fake GATT plus an in-memory USB/IP
peer exercises initialization/pairing, input/output, disconnect, cache and lifecycle.
Release measurements on the recorded Linux environment achieved sub-0.5 ms p99
and zero measured steady-state allocations; this does not establish real controller
or kernel latency. Reproduce with:

```sh
cargo test --release -p ns2pro-bridge --test linux_latency -- --ignored --nocapture --test-threads=1
```

User-operated Linux x64 runs confirm native BLE input, vhci USB enumeration and
Steam input with SteamRT3 disabled after the legacy client dependency fix above.
Run the executable built in the same checkout; startup logs now identify its
absolute path, so stale binaries in a different worktree can be distinguished.
The earlier baseline capture used 48.75 ms (roughly 20 Hz); the later startup LE
update confirms 7.5 ms on the tested adapter, with user-confirmed delay improvement
and power-off/reconnect. No independent 7.5 ms packet capture is archived. Users also
confirmed motion, basic rumble and steady/restored player indicators. Firmware,
detailed input/power/output, cleanup/coexistence, long-duration stability, Windows
physical and ARM hardware acceptance remain incomplete. Follow the [hardware checklist](docs/linux-hardware-checklist.md),
[Linux evidence](docs/rust-linux-implementation.md), and
[integration record](docs/rust-integration-validation.md).

## License and attribution

GPL-3.0-or-later; see [LICENSE.txt](LICENSE.txt) and
[THIRD_PARTY.md](THIRD_PARTY.md). USB behavior and fixtures derive from VIIPER;
protocol research credits include
[switch2_controller_research](https://github.com/ndeadly/switch2_controller_research).
