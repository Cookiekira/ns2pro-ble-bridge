pub mod host;
pub mod server;
pub mod session;

#[cfg(target_os = "linux")]
pub mod vhci;
#[cfg(windows)]
pub mod usbip_win;
// Exercise the native driver's byte contracts in Linux CI as well.
#[cfg(all(test, not(windows)))]
#[path = "usbip_win_wire.rs"]
mod usbip_win_wire_tests;
