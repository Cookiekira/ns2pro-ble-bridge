//! Pure device and USB/IP engine. No runtime or clock dependencies.
pub mod descriptors;
pub mod device;
pub mod engine;
pub mod usbip;
pub use device::{InputSample, Ns2ProDevice, Output};
