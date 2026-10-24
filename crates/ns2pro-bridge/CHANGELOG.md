# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1](https://github.com/Cookiekira/ns2pro-bridge/releases/tag/v0.3.1) - 2026-10-24

### Added

- expose controller flash identity over USB
- *(cli)* describe options in --help
- *(stats)* record per-connection BLE interval evidence
- *(linux)* request live 7.5ms LE connection interval

### Fixed

- wait for controller identity before auto-attach
- log successful host pairing with controller address
- *(linux)* report vhci EOF as kernel controller removal
- *(linux)* require effective CAP_NET_RAW for HCI observer
- *(stats)* log observed BLE interval in milliseconds
- *(usb)* retain unread bulk response bytes across host transfers
- *(linux)* preserve writable readiness after ATT connect

### Other

- Automate releases with cargo-dist and release-plz
- Tidy vhci setup placement and attach hint
- Simplify root invariant and platform setup
- Simplify elevation command construction
- Support binstall and automatic Linux elevation
- Merge remote-tracking branch 'origin/main' into refactor/deepen-bridge-modules
- *(controller)* record the connection report on establishment
- *(controller)* inline the single-use input sink
- *(usb)* fold lease detach into the attachment adapter
- *(ble)* inline single-use target and characteristic helpers
- Merge branch 'refactor/link-teardown' into refactor/architecture-integration
- Merge branch 'refactor/usb-attachment' into refactor/architecture-integration
- *(usb)* remove late lease test polling order assumption
- *(controller)* own connection teardown and late input guard
- Merge branch 'refactor/usb-attachment' into refactor/architecture-integration
- Merge branch 'refactor/cache-selection' into refactor/architecture-integration
- Merge branch 'refactor/statistics-interface' into refactor/architecture-integration
- Merge branch 'refactor/portable-ble-policy' into refactor/architecture-integration
- *(usb)* encapsulate attachment lifecycle and lease cleanup
- *(ble)* share portable scan and discovery policy
- *(tests)* share portable fake controller
- *(cache)* own target selection and attempt invalidation
- *(stats)* encapsulate statistics behind event methods
- require root on Linux and drop setcap/capability handling
- *(linux)* consume USB transport before detaching lease
- *(ble)* use typed LE address kinds across transports
- *(linux)* route HCI observer syscalls through socket helpers
- *(linux)* encapsulate optional interval observer lifecycle
- *(linux)* model interval observation as explicit connection states
- *(proto)* centralize LE interval wire format and parameters
- *(windows)* consolidate watchers and GATT resource ownership
- *(proto)* reuse defaults and remove unused advertisement helpers
- *(app)* share platform composition and type controller loops
- *(session)* return attempt outcomes and reuse supervisor cache
- *(usb)* centralize management parsing and streamline URB serving
- *(usb)* share usbip-win codec and block on worker cancellation
- *(linux)* track scan cleanup and centralize management parsing
- *(linux)* share Bluetooth socket syscall helpers
- *(linux)* unify ATT transactions and protocol encoding
- document abstraction ablations and retain ATT transaction protection
- remove unused retry and binary-layout dependencies
- *(usb)* remove the error-only host module
- *(linux)* rely on ATT packet atomicity instead of a writer lock
- *(controller)* serialize commands through exclusive ownership
- log executable path and controller connection milestones
- Prevent late notification callbacks from overwriting neutral disconnect state
- Cover pairing, cache, retry and rumble behavior through complete sessions
- Finalize Windows validation evidence and remove duplicate integration module declaration
- Merge commit '86a7085' into feat/rust-rewrite
- Merge branch 'feat/rust-windows' into feat/rust-rewrite
- Keep CLI validation helpers before test module for strict Windows checks
- Measure allocation-free input latency and decode native mgmt permission errors
- Verify native event object lifetime without racing handle-number reuse
- Honor Windows attach cancellation before driver discovery and during failure cleanup
- Merge branch 'feat/rust-windows' into feat/rust-rewrite
- Drain native shutdown and correlate latency with the completed input sample
- Merge application composition and implement native Windows Bluetooth smoke scan
- Merge commit 'c8135e1' into feat/rust-rewrite
- Merge branch 'feat/rust-rewrite' into feat/rust-linux
- Harden Linux cancellation, ATT negotiation and reconnect lifecycle tests
- Compose native controller and USB lifecycles on a current-thread runtime
- Integrate native Windows Bluetooth and usbip-win2 with Linux modules
- Record passing Windows shared-session tests and strict target checks
- Merge shared session harness and retain both native platform declarations
- Verify fake GATT to USB sessions and correct established-session cleanup
- Defer Windows adapter identity lookup until requested by shared session
- Wire Windows modules and test buffer borrowing, IOCTL errors and resource cleanup
- Wire and validate Linux modules against the common bridge foundation
- Merge commit '44fdbcb' into feat/rust-windows
- Implement native Windows WinRT Bluetooth and cancellable usbip-win2 attach
- Merge branch 'feat/rust-rewrite' into feat/rust-linux
- Implement native Linux mgmt discovery, raw AsyncFd ATT and vhci attachment
- Implement shared protocol pairing, USB device engine and portable bridge sessions
- Establish native Bluetooth and USB attach contracts for parallel platform work
- preserve Rust rewrite specification and unfinished seed
