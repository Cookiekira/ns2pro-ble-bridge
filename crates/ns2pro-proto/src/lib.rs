//! Pure Switch 2 Pro Controller protocol.
//!
//! Nothing in this crate performs I/O, spawns tasks, reads clocks or draws
//! randomness. Every type is a value or a state machine that is driven by the
//! I/O shell in `ns2pro-bridge`, which keeps the protocol testable without
//! hardware.

pub mod address;
pub mod advertisement;
pub mod att;
pub mod calibration;
pub mod command;
pub mod gatt;
pub mod input;
pub mod mgmt;
pub mod pairing;

pub use address::Address;
pub use input::{Buttons, InputState, Stick};
