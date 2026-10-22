# Linux hardware acceptance

Record revision, distribution/kernel, architecture, adapter model/firmware,
controller firmware, privileges and whether bluetoothd is running for every run.
The final x64 acceptance used the published `v0.2.0` binary; see
[v0.2.0 acceptance run](#2026-10-22-v020-acceptance-run). Earlier user-operated
runs are kept as history; see [integration evidence](rust-integration-validation.md).

## Prerequisites

- [x] Root-operated auto-attach and live interval request with the published `v0.2.0` binary (runs A–F below). Non-root capability setup (`setcap`) is not verified.
- [x] Usable LE adapter (MediaTek MT7922, USB `0e8d:0616`) and `vhci-hcd` loaded.
- [x] No other active controller session: an older pre-merge bridge was stopped before run A.

## Native and USB path

- [x] Hidden `--bt-smoke-test` selects adapter `EC:91:61:88:1C:C4` and exits 0 (run S). It returns on the first controller advertisement or succeeds when the 500 ms window ends.
- [x] Discovery identifies the controller advertisement and LE address type; cached runs connect only after a fresh advertisement (runs C, D, F).
- [x] Controller initialization succeeds (all runs).
- [x] First pairing with `--forget-device` and an empty cache file succeeds and writes `{"address":"78:81:8C:E8:54:3F"}` (run A). Re-pairing after cache invalidation also succeeds (run E). Pairing emits no dedicated log line; success is shown by initialization without a cached address.
- [x] AF_UNIX socketpair attaches through vhci sysfs: `hs 0000 006 002 00010001 000009 8-1` (run A).
- [x] TCP-fd fallback not needed: the kernel accepted AF_UNIX; no `vhci rejected AF_UNIX` warning. The fallback remains covered by unit tests only.
- [x] Controller identity (#30, PR #32 build `c9c5221`): the bridge read flash block `0x13000` from the controller, attached vhci only afterwards (log: interval update 12:54:44.379 → `virtual USB controller attached` 12:54:44.823 → initialized 12:54:45.085), and Steam shows `Device Support: HEW700072169 (0x057e,0x2069)` instead of `VIIPERNS2PRO`.
- [x] Virtual `057e:2069` enumerates at Full Speed 12M, `bcdUSB 2.00`, interfaces HID (3) and vendor (255); Steam opens it (`usbfs`) (run A).
- [x] User confirms all buttons, both stick ranges and centering, battery level and charging state, and player-LED changes in Steam (run A).
- [x] User confirms gyroscope/motion control works. Sensor timestamp semantics remain the monotonic receipt fallback (see Timing).
- [x] Rumble reaches the controller; stats record rumble writes (`rumble_hz≈0.21` over run A). Latest-value 20 ms throttling is covered by session tests and kept at 20 ms.
- [x] User confirms player indicators restore after reconnect.
- [x] User confirms host-initiated disconnect publishes neutral input and the controller reconnects; no stale rumble replays. Bulk/IN/UNLINK behavior is covered by session tests.
- [x] `--forget-device` (run A), cached address (runs C, D, F) and failed-cache rediscovery (run E) behave correctly. Explicit `--device-address`, `--host-address` and `--pair-host` across a reconnect are covered by session tests, not separately run on hardware.
- [x] Failed cached setup invalidates the cache: a fake cached address timed out after 30 s, the cache was removed, the bridge retried after ~1.6 s, rediscovered, re-paired and rewrote the real address (run E). Cancellation preserving the cache is covered by session tests.
- [x] Ctrl+C during steady use releases the controller and frees the vhci port; the kernel logs `USB disconnect` (runs A, B).
- [x] Kernel-initiated USB device loss is fatal: writing the port to `vhci_hcd.0/detach` made the bridge exit with `USB transport ended: early eof` and no reattach (run C). The message is less specific than intended; see follow-ups.
- [x] Manual TCP attachment with `--no-auto-attach`: server on the default `localhost:3241` (resolved to `[::1]:3241` on this host), `usbip --tcp-port 3241 attach -r localhost -b 1-1` attached busid `1-1`, the device enumerated and URBs flowed (run F). IPv4 clients need `--usb-addr 127.0.0.1:3241` or `0.0.0.0:3241` when `localhost` resolves only to `::1`.
- [x] bluetoothd running (runs A, E, F) and masked/stopped (run D) both work. With bluetoothd running, stopping owned discovery after a timeout once returned mgmt status `0x0b` (warning only; run E). bluetoothd is D-Bus activated by PipeWire, so a plain `systemctl stop` is not enough to test without it; use `systemctl mask --runtime bluetooth`.

## Timing and architecture

- [x] Captured Nintendo LE connection event: 48.75 ms, latency 0, supervision 420 ms before the request (every run).
- [x] Fresh HCI capture (run A, `session-a.btsnoop`, sha256 `3e0325168c58dff767f796a0902b8eab1bf8a34965926cdec3121bf8ce648942`, kept locally because it contains other device addresses):
  ```
  < HCI Command: LE Connection Update (0x08|0x0013) plen 14
        Handle: 513 (LE-ACL) Address: 78:81:8C:E8:54:3F (Nintendo Co.,Ltd)
        Min/Max connection interval: 7.50 msec (0x0006)
        Connection latency: 0 (0x0000)
        Supervision timeout: 2000 msec (0x00c8)
  > HCI Event: Command Status (0x0f) plen 4
      LE Connection Update (0x08|0x0013) ncmd 1  Status: Success (0x00)
  > HCI Event: LE Meta Event (0x3e) plen 10
      LE Connection Update Complete (0x03)  Status: Success (0x00)
        Handle: 513  Connection interval: 7.50 msec  latency 0  Supervision timeout: 2000 msec
  ```
- [x] Stats report target, preference result and observed interval: `target_interval=Interval(6) preference_result=Confirmed observed_interval_ms=7.5`, mean interarrival 7.51–7.54 ms, BLE report rate ~123–130 Hz, bridge p50 ~6.6–6.8 µs, p99 ~24–50 µs.
- [x] Optimization failure keeps the connection: with `CAP_NET_RAW` dropped (`capsh --drop=cap_net_raw`), the request was not sent, input continued at the system interval (~48 ms mean interarrival), and stats reported `preference_result=Unconfirmed observed_interval_ms=unknown` (run C). The warning names an epoch mismatch instead of the missing capability; see follow-ups.
- [ ] Verified 5 ms hardware path: unavailable on this adapter; generic interval 4 is never sent.
- [x] Controller IMU timestamp semantics not verified; the monotonic receipt-time fallback is retained as the spec allows.
- [x] Linux x64 hardware acceptance (this record). Linux arm64: native CI build and tests only (run 39772269154); no arm64 hardware available.
- [ ] Long soak on `v0.2.0`: runs A–F were each a few minutes. A one-hour play session on the pre-merge build had no reported problems; a 30-minute `v0.2.0` soak remains to be recorded.

Windows delivery has separate build/test evidence; no Windows hardware claim is made.

## 2026-10-22 v0.2.0 acceptance run

- Binary: `ns2pro-ble-bridge-v0.2.0-linux-x64` from the [v0.2.0 release](https://github.com/Cookiekira/ns2pro-ble-bridge/releases/tag/v0.2.0), sha256 `852a4cac4a6b6d07e6a8bf6cbfe055447d17de09d224df3b1eb47a08730cb18f`, `--version` 0.2.0.
- Host: CachyOS, kernel `7.3.0-rc6-2-cachyos-rc` x86_64, run with `sudo`.
- Adapter: MediaTek MT7922 Bluetooth (USB `0e8d:0616`, `btusb`), firmware build time `20260724143815`, HCI version 13, manufacturer 70, address `EC:91:61:88:1C:C4`.
- Controller: Switch 2 Pro Controller `78:81:8C:E8:54:3F`; firmware version unavailable on Linux.

| Run | Command (abridged) | bluetoothd | Result |
| --- | --- | --- | --- |
| A | `--forget-device --cache-file cache.json --log-level debug`, btmon capturing | active | Pairing, AF_UNIX attach, 7.5 ms (log and HCI capture), input/LED/battery/rumble checks, Ctrl+C cleanup |
| B | same, restarted | active | 7.5 ms, user-confirmed disconnect/neutral/reconnect, Ctrl+C cleanup |
| S | `--bt-smoke-test` | active | exit 0 |
| C | `capsh --drop=cap_net_raw`, cached | restarted by D-Bus | Connection continues without interval request; kernel detach makes the bridge exit |
| D | cached | masked and inactive | Discovery, connection, 7.5 ms, input |
| E | fake cached address | active | 30 s timeout, cache invalidated, rediscovery, re-pair, cache rewritten |
| F | `--no-auto-attach`, cached | active | `usbip attach -r localhost -b 1-1` enumerates and transfers |

Follow-ups (diagnostics/docs only, no acceptance blocker):

1. Missing `CAP_NET_RAW` still opens an untrusted HCI socket, so the interval warning reports an epoch mismatch rather than the missing capability.
2. Kernel-initiated vhci removal ends with `USB transport ended: early eof` instead of the intended `kernel removed virtual controller` message.
3. README manual-attach examples should note that `localhost` may resolve only to `::1`.
4. Pairing has no dedicated log line.
