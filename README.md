# NS2Pro BLE Bridge

Use a **Switch 2 Pro Controller over Bluetooth** as a wired Switch 2 Pro Controller (USB `057e:2069`) on Linux or Windows. Steam and SDL see a normal USB controller.

## Start here (Linux)

1. Download `ns2pro-ble-bridge-<version>-linux-x64` (or `-linux-arm64`) from [Releases](https://github.com/Cookiekira/ns2pro-ble-bridge/releases/latest), then run `chmod +x` on it.
2. Load the virtual USB driver: `sudo modprobe vhci-hcd`
3. Start the bridge: `sudo ./ns2pro-ble-bridge-<version>-linux-x64`
4. **First time only:** press the controller's **sync button** (top edge) to pair. The bridge remembers it.
5. Wait for `controller initialized; receiving input`. Open Steam and play.

Next time, run step 3 and press **Home** to wake the controller. Stop with **Ctrl+C**; the virtual controller detaches cleanly.

Nothing else to install: no `usbip` tools, no BlueZ utilities, no .NET, Go or VIIPER. `bluetoothd` may keep running.

## Windows

1. Install the signed [usbip-win2](https://github.com/vadimgrn/usbip-win2) driver (supported: v0.9.6.1–v0.9.8.1).
2. Download `ns2pro-ble-bridge-<version>-win-x64.exe` from [Releases](https://github.com/Cookiekira/ns2pro-ble-bridge/releases/latest).
3. Run it, then press the controller's **sync button** the first time.

Windows is verified by native CI build and tests only. It has not been tested on real Windows Bluetooth or driver hardware yet.

## If something goes wrong

| You see | Fix |
| --- | --- |
| Permission error at startup | Run with `sudo`. |
| `vhci` / attach error | `sudo modprobe vhci-hcd`, then start again. |
| Steam shows the controller but no input (Linux, 32-bit Steam) | `sudo pacman -S --needed lib32-libusb` (Arch/CachyOS; use your distro's 32-bit libusb), then fully restart Steam. |
| `kernel removed virtual controller` | The virtual USB device was detached. Start the bridge again. |

Debug details: add `--log-level debug`. Every 5 s the log shows report rate, latency p50/p99 and the actual connection interval.

## Use the controller on another machine

1. On the machine with Bluetooth: `sudo ./ns2pro-ble-bridge-<version>-linux-x64 --no-auto-attach --usb-addr 0.0.0.0:3241`
2. On the other machine: `sudo usbip --tcp-port 3241 attach -r HOST -b 1-1`

The USB/IP server has no authentication: expose it only to trusted machines.

## Options

| Option | Default | What it does |
| --- | --- | --- |
| `--forget-device` | off | Forget the saved controller and pair again |
| `--device-address MAC` | cache | Connect to this controller address |
| `--no-auto-attach` | off | Serve USB/IP over TCP instead of attaching locally |
| `--usb-addr HOST:PORT` | `localhost:3241` | USB/IP server address |
| `--log-level LEVEL` | `info` | `trace`/`debug`/`info`/`warn`/`error`/`off`; `RUST_LOG` wins |

More options:

| Option | Default | What it does |
| --- | --- | --- |
| `--pair-host` | off | Re-pair the host once per run |
| `--host-address MAC` | adapter | Host address used for pairing |
| `--cache-file PATH` | local app data `Ns2Pro.BleBridge/controller-cache.json` | Saved controller, JSON `{"address":"AA:BB:CC:DD:EE:FF"}` |
| `--feature-flags FLAGS` | `0x07` | Buttons, sticks, IMU (hex or decimal) |

Exit codes: `0` normal, `1` runtime failure, `2` bad arguments or USB setup failure. Options are case-sensitive; there is no `/?` alias.

## What to expect

- **Latency:** the bridge itself adds about 25–50 µs (p99) on the tested machine.
- **Bluetooth interval:** Linux asks for **7.5 ms** (default is often 48.75 ms) and logs the result. 5 ms is not supported on any verified adapter.
- **Disconnects:** buttons and sticks return to neutral immediately. The bridge reconnects automatically, restores player LEDs and drops old rumble.
- **Tested hardware:** Linux x64 with a MediaTek MT7922 adapter. arm64 and Windows: CI only.

## Build from source

```sh
cargo build --locked --release -p ns2pro-bridge
cargo test --locked --workspace
```

Output: `target/release/ns2pro-ble-bridge` (`.exe` on Windows). Needs stable Rust (edition 2024).

## Credits and license

- **[VIIPER](https://github.com/Alia5/VIIPER)** by Alia5 (GPL-3.0): the virtual USB device model, descriptors and test fixtures are ported from VIIPER. Its license is in [LICENSES/VIIPER-GPL-3.0.txt](LICENSES/VIIPER-GPL-3.0.txt).
- **[switch2_controller_research](https://github.com/ndeadly/switch2_controller_research)**: controller protocol research.
- **[usbip-win2](https://github.com/vadimgrn/usbip-win2)**: Windows USB/IP driver (installed separately).

This project is GPL-3.0-or-later; see [LICENSE.txt](LICENSE.txt) and [THIRD_PARTY.md](THIRD_PARTY.md).
