//! Latest-value state shared by callbacks, USB and controller sessions.
use crate::stats::Statistics;
use ns2pro_usb::{InputSample, Output};
use std::sync::{Arc, Mutex};
use tokio::sync::watch;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rumble {
    pub left: [u8; 16],
    pub right: [u8; 16],
}
#[derive(Clone, Debug)]
pub struct Channels {
    pub input: watch::Sender<InputSample>,
    pub leds: watch::Sender<u8>,
    pub rumble: watch::Sender<Option<Rumble>>,
    pub stats: Arc<Mutex<Statistics>>,
}
impl Default for Channels {
    fn default() -> Self {
        Self {
            input: watch::channel(InputSample::default()).0,
            leds: watch::channel(6).0,
            rumble: watch::channel(None).0,
            stats: Arc::new(Mutex::new(Statistics::default())),
        }
    }
}
impl Channels {
    pub fn output(&self, output: Output) {
        match output {
            Output::PlayerLeds(v) => {
                self.leds.send_replace(v);
            }
            Output::Rumble { left, right } => {
                self.rumble.send_replace(Some(Rumble { left, right }));
            }
        }
    }
    pub fn neutral(&self) {
        self.input.send_modify(|s| {
            s.state = Default::default();
            s.seq = s.seq.wrapping_add(1);
            s.motion_timestamp_us = 0;
        });
        self.rumble.send_replace(None);
    }
}
