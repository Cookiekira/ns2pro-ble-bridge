mod attachment;
pub use attachment::{Attachment, Error};
pub mod server;
pub mod session;

#[cfg(windows)]
pub mod usbip_win;
#[cfg(target_os = "linux")]
pub mod vhci;
#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("USB attach I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("USB attach: {0}")]
    Platform(String),
    #[error("USB attach cancelled")]
    Cancelled,
}
