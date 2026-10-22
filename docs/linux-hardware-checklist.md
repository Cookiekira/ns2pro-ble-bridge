# Linux hardware acceptance checklist

Record the bridge revision, Linux distribution/kernel, BlueZ version, CPU architecture,
Bluetooth adapter, and controller firmware before starting.

## Prerequisites

- [ ] BlueZ is running and the adapter is powered.
- [ ] The `usbip` command is installed.
- [ ] `vhci-hcd` is loaded (`lsmod | rg vhci_hcd`).
- [ ] The bridge can auto-attach with the chosen privileges, or `--no-auto-attach` is used.
- [ ] No other process has an active connection to the controller.

## Controller and virtual USB

- [ ] First pairing discovers the controller and completes host pairing.
- [ ] A later run reconnects using the cached controller without pairing mode.
- [ ] Buttons, both sticks, and IMU motion appear on the virtual NS2Pro device.
- [ ] Rumble from the virtual device is reproduced by the physical controller.
- [ ] Player LED changes from the virtual device reach the physical controller.
- [ ] Powering off or moving the controller out of range ends the session and reconnects after return.
- [ ] `--forget-device` clears the cache and the next run returns to discovery.
- [ ] `--pair-host` repeats pairing for a cached controller.
- [ ] `--device-address` connects to the specified controller.
- [ ] `--host-address` overrides the BlueZ adapter address used for pairing.
- [ ] `usbip list -r localhost` shows the export and the attached device is visible to `lsusb`.
- [ ] `--no-auto-attach` permits a manual or remote USB/IP attach.

## Architectures

- [ ] The `linux-x64` release artifact completes the checklist on x64 hardware.
- [ ] The `linux-arm64` release artifact completes the checklist on ARM64 hardware.
