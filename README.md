# NS2Pro BLE Bridge

Windows CLI for bridging a Switch 2 Pro Controller over BLE to a virtual USB NS2Pro device.

## Requirements

- Windows x64
- .NET SDK with `net10.0-windows` support
- Go toolchain
- VIIPER source checkout

## Build

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

## Usage

```powershell
.\bin\Release\net10.0-windows\win-x64\publish\Ns2Pro.BleBridge.exe
```

Options:

| Option | Description | Default |
| --- | --- | --- |
| `--usb-addr <addr>` | USB server address | `localhost:3241` |
| `--device-address <mac>` | Target BLE controller address | Cached controller |
| `--pair-host` | Pair the BLE controller to the host | Disabled |
| `--host-address <mac>` | Host Bluetooth address for pairing | Required with `--pair-host` |
| `--forget-device` | Clear the cached BLE controller address | Disabled |
| `--cache-file <path>` | Controller cache path | `%LOCALAPPDATA%\Ns2Pro.BleBridge\controller-cache.json` |
| `--no-auto-attach` | Do not automatically attach to the local USB bus | Disabled |
| `--feature-flags <flags>` | Feature flags to enable | `0x07` |
| `--log-level <level>` | Log level: `Trace`, `Debug`, `Info`, `Warn`, `Error` | `Info` |
| `-h`, `--help` | Show help | |

## License

Licensed under GPL-3.0-or-later. See [LICENSE.txt](LICENSE.txt).
