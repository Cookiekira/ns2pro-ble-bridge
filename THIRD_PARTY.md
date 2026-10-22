# Attribution

The USB device descriptors, reports, command handling, synthetic flash and golden
fixtures derive from [VIIPER](https://github.com/Alia5/VIIPER), by Alia5 and its
contributors, under GPL-3.0. The reference revision recorded by the former gitlink
is `70e533386271ef3f28a2e0ce0235ab0060d8c336` in
[Cookiekira/VIIPER](https://github.com/Cookiekira/VIIPER). Its license is retained
in [LICENSES/VIIPER-GPL-3.0.txt](LICENSES/VIIPER-GPL-3.0.txt). The exported bytes
remain in `fixtures/viiper`; the Rust golden tests preserve the byte contracts.
No VIIPER runtime or build dependency is required.

The NS2Pro protocol and pairing exchange were ported from this project's original
C# implementation, informed by
[switch2_controller_research](https://github.com/ndeadly/switch2_controller_research).
Native Windows driver wire layouts follow
[usbip-win2](https://github.com/vadimgrn/usbip-win2); see
[Windows implementation evidence](docs/windows-native-validation.md) for supported
released ABI versions and primary source links.

This project is GPL-3.0-or-later; see [LICENSE.txt](LICENSE.txt).
