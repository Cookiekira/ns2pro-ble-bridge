#[derive(Debug, thiserror::Error)]
pub enum AttachError {
    #[error("USB attach I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("USB attach: {0}")]
    Platform(String),
    #[error("USB attach cancelled")]
    Cancelled,
}
