# NS2Pro BLE Bridge

Windows CLI for bridging a Switch 2 Pro Controller over BLE to a virtual USB NS2Pro device.

## Requirements

- Windows x64
- [usbip-win2](https://github.com/vadimgrn/usbip-win2) installed on the system

## Usage

Install `usbip-win2` first; the bridge uses it to attach the virtual USB NS2Pro
device to the local Windows USB bus.

Download `Ns2Pro.BleBridge-v0.1.1-win-x64.exe` from the
[latest GitHub Release](https://github.com/Cookiekira/ns2pro-ble-bridge/releases/latest).
Put the controller into Bluetooth pairing mode, then run the downloaded executable:

```powershell
.\Ns2Pro.BleBridge-v0.1.1-win-x64.exe
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
| `--cache-file <path>` | Controller cache path | `%LOCALAPPDATA%\Ns2Pro.BleBridge\controller-cache.json` |
| `--no-auto-attach` | Do not automatically attach to the local USB bus | Disabled |
| `--feature-flags <flags>` | Feature flags to enable | `0x07` |
| `--log-level <level>` | Log level: `Trace`, `Debug`, `Info`, `Warn`, `Error` | `Info` |
| `-h`, `--help` | Show help | |

## Build from source

Build requirements:

- .NET SDK with `net10.0-windows` support
- Go toolchain
- VIIPER source checkout

Set the path to the VIIPER source checkout, then publish the bridge:

```powershell
$env:VIIPER_SOURCE_ROOT = "C:\path\to\VIIPER"
dotnet publish .\Ns2Pro.BleBridge.csproj -c Release -r win-x64
```

You can also pass the VIIPER path as an MSBuild property:

```powershell
dotnet publish .\Ns2Pro.BleBridge.csproj -c Release -r win-x64 /p:ViiperSourceRoot=C:\path\to\VIIPER
```

The build compiles `lib\viiper` as `libVIIPER.dll` and embeds it in the published executable.

## Credits

Thanks to [switch2_controller_research](https://github.com/ndeadly/switch2_controller_research)
for the Switch 2 controller protocol research and [VIIPER](https://github.com/Cookiekira/VIIPER)
for the native virtual USB backend.

## License

Licensed under GPL-3.0-or-later. See [LICENSE.txt](LICENSE.txt).
