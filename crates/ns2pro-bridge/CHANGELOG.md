# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.3.1](https://github.com/Cookiekira/ns2pro-bridge/releases/tag/v0.3.1) - 2026-10-24

### Added

- Install with `cargo binstall ns2pro-bridge` or the shell/PowerShell installers; releases ship checksums.
- Linux: running as a normal user prompts for the password (`sudo`, or `pkexec` without a terminal).
- Linux: `vhci-hcd` is loaded automatically when auto-attach needs it.
- Linux: SIGTERM and SIGHUP detach the virtual controller cleanly, like Ctrl+C.

### Changed

- Renamed to `ns2pro-bridge` (executable, repository and release assets). Release assets are now `ns2pro-bridge-<target>.tar.xz`/`.zip`.
- The saved controller moved to `/var/lib/ns2pro-bridge/` on Linux and `<local app data>/ns2pro-bridge/` on Windows. Pair once again after upgrading.
- Linux release binaries need glibc ≥ 2.39.
