# NS2Pro BLE Bridge

Cross-platform CLI for bridging a Switch 2 Pro Controller over BLE to a virtual USB NS2Pro device.

## Requirements

- Windows x64 with [usbip-win2](https://github.com/vadimgrn/usbip-win2), or
- Linux x64/ARM64 with BlueZ, the distribution's USB/IP tools, and the `vhci-hcd` kernel module

## Usage

Install the platform USB/IP prerequisites first; the bridge uses them to attach the
virtual USB NS2Pro device to the local USB bus. Linux setup is described in
[Linux setup](#linux-setup).

Download `Ns2Pro.BleBridge-v0.1.1-win-x64.exe` from the
[latest GitHub Release](https://github.com/Cookiekira/ns2pro-ble-bridge/releases/latest).
Put the controller into Bluetooth pairing mode, then run the downloaded executable.
On Windows:

```powershell
.\Ns2Pro.BleBridge-v0.1.1-win-x64.exe
```

On Linux:

```bash
chmod +x Ns2Pro.BleBridge-v0.1.1-linux-x64
./Ns2Pro.BleBridge-v0.1.1-linux-x64
```

On first use, the bridge scans for a pairing Switch 2 Pro Controller, connects to it,
pairs it to the local Bluetooth adapter, and caches the controller address. Later runs
reuse the cached controller. If the cached controller no longer connects, the cache is
cleared automatically so the next retry scans again.

Options:

| Option | Description | Default |
| --- | --- | --- |
| `--usb-addr <addr>` | USB server address | `localhost:3241` |
| `--device-address <mac>` | Connect to a specific BLE controller (debug override) | Auto-discovered or cached controller |
| `--pair-host` | Pair a cached or explicit controller again | Disabled |
| `--host-address <mac>` | Override local Bluetooth adapter address for host pairing | Auto-detected |
| `--forget-device` | Clear the cached BLE controller address | Disabled |
| `--cache-file <path>` | Controller cache path | Platform local application-data directory |
| `--no-auto-attach` | Do not automatically attach to the local USB bus | Disabled |
| `--feature-flags <flags>` | Feature flags to enable | `0x07` |
| `--log-level <level>` | Log level: `Trace`, `Debug`, `Info`, `Warn`, `Error` | `Info` |
| `-h`, `--help` | Show help | |

## Linux setup

The bridge talks to BlueZ over the system D-Bus and keeps VIIPER in-process. It does
not require a separate VIIPER server. Ensure Bluetooth is enabled and the user can
access the system BlueZ service.

Install USB/IP and load its virtual host-controller driver:

```bash
# Ubuntu/Debian
sudo apt install bluez linux-tools-generic

# Fedora
sudo dnf install bluez usbip

# Arch Linux
sudo pacman -S bluez bluez-utils usbip

sudo modprobe vhci-hcd
```

The automatic local USB/IP attach normally needs elevated permission. Run the bridge
with appropriate local policy/capabilities, or use `--no-auto-attach` and attach the
export manually or from a remote USB/IP client. A typical manual local flow is:

```bash
./Ns2Pro.BleBridge-v0.1.1-linux-x64 --no-auto-attach
sudo usbip attach -r localhost -b <bus-id>
```

On first use, put the controller in Bluetooth pairing mode. The bridge discovers it,
runs the NS2Pro host-pairing exchange using the selected adapter's address, and stores
the controller address. Later runs reconnect from that cache. Use `--pair-host` to
repeat host pairing, `--forget-device` to clear the cache, or `--device-address` and
`--host-address` for explicit overrides.

For hardware validation, follow the [Linux manual acceptance checklist](docs/linux-hardware-checklist.md).

## Build from source

Build requirements:

- .NET10 SDK
- Windows SDK `10.0.26100.0` when publishing `win-x64`
- A C compiler supported by Go's `c-shared` build mode
- Go toolchain
- VIIPER submodule initialized

Clone the repository with submodules, then publish the bridge:

```bash
git clone --recurse-submodules https://github.com/Cookiekira/ns2pro-ble-bridge.git
cd ns2pro-ble-bridge
dotnet publish Ns2Pro.BleBridge.csproj -c Release -r linux-x64
dotnet publish Ns2Pro.BleBridge.csproj -c Release -r linux-arm64
# On Windows:
dotnet publish Ns2Pro.BleBridge.csproj -c Release -r win-x64
```

If you already cloned the repository without submodules, initialize VIIPER before
building:

```powershell
git submodule update --init --recursive
```

By default, the build uses `vendor/VIIPER`. To build against a different VIIPER
checkout, set `VIIPER_SOURCE_ROOT` or pass the path as an MSBuild property:

```bash
VIIPER_SOURCE_ROOT=/path/to/VIIPER dotnet publish Ns2Pro.BleBridge.csproj -c Release -r linux-x64
dotnet publish Ns2Pro.BleBridge.csproj -c Release -r linux-x64 /p:ViiperSourceRoot=/path/to/VIIPER
```

The build compiles `lib/viiper` as `libVIIPER.dll` on Windows or `libVIIPER.so` on
Linux and embeds the platform library in the published executable.

## Credits

Thanks to [switch2_controller_research](https://github.com/ndeadly/switch2_controller_research)
for the Switch 2 controller protocol research and [VIIPER](https://github.com/Cookiekira/VIIPER)
for the native virtual USB backend.

## License

Licensed under GPL-3.0-or-later. See [LICENSE.txt](LICENSE.txt).
