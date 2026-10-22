pub mod server;
pub mod session;

#[cfg(windows)]
pub mod usbip_win;
#[cfg(target_os = "linux")]
pub mod vhci;
// Exercise the native driver's byte contracts in Linux CI as well.
#[cfg(all(test, not(windows)))]
#[path = "usbip_win_wire.rs"]
mod usbip_win_wire_tests;

#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("USB attach I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("USB attach: {0}")]
    Platform(String),
    #[error("USB attach cancelled")]
    Cancelled,
}
