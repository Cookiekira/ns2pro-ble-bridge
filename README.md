# NS2Pro BLE Bridge

NativeAOT Windows CLI that bridges a real Switch 2 Pro Controller over BLE into a virtual USB NS2Pro device through an embedded `libVIIPER.dll`.

## Build

```powershell
cd C:\Users\oddc2\Repo\VIIPER
set CGO_ENABLED=1
go build -buildmode=c-shared -o dist\libVIIPER\libVIIPER.dll .\lib\viiper

cd C:\Users\oddc2\Repo\ns2pro-ble-bridge
dotnet publish .\Ns2Pro.BleBridge.csproj -c Release -r win-x64
```

The project embeds `..\VIIPER\dist\libVIIPER\libVIIPER.dll` into the NativeAOT executable. At runtime it extracts the DLL into `%LOCALAPPDATA%\Ns2Pro.BleBridge\native\<hash>\` and loads it from there.

## Run

```powershell
.\bin\Release\net10.0-windows10.0.26100.0\win-x64\publish\Ns2Pro.BleBridge.exe
```

Useful flags:

```text
--usb-addr localhost:3241
--device-address AA:BB:CC:DD:EE:FF
--pair-host --host-address AA:BB:CC:DD:EE:FF
--forget-device
--cache-file path\to\ns2pro_ble_device.json
--no-auto-attach
--feature-flags 0x07
--log-level debug
```

Startup order intentionally creates and auto-attaches the virtual USB NS2Pro before scanning or connecting to the BLE controller, so Steam sees the NS2Pro identity first.
