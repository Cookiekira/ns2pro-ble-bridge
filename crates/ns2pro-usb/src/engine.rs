//! Event-driven URB scheduler; caller supplies monotonic microseconds.
use crate::{
    InputSample, Ns2ProDevice, Output,
    usbip::{Urb, reply_into},
};
use std::collections::VecDeque;
#[derive(Debug, Default)]
pub struct Effects {
    pub replies: Vec<u8>,
    pub outputs: Vec<Output>,
    pub input_completions: u64,
    pub keepalives: u64,
}
#[derive(Debug)]
pub struct UrbEngine {
    pub device: Ns2ProDevice,
    sample: InputSample,
    dirty: bool,
    pending: VecDeque<Urb>,
    last_report: u64,
}
impl Default for UrbEngine {
    fn default() -> Self {
        Self {
            device: Ns2ProDevice::default(),
            sample: InputSample::default(),
            dirty: true,
            pending: VecDeque::new(),
            last_report: 0,
        }
    }
}
impl UrbEngine {
    pub fn submit(&mut self, u: Urb, data: &[u8], now: u64, e: &mut Effects) {
        if u.command == 2 {
            self.unlink(u, e);
            return;
        }
        let mut status = 0;
        let mut payload = Vec::new();
        let mut output = None;
        match (u.direction, u.ep) {
            (_, 0) => match self.device.control(u.setup, data, &self.sample) {
                Ok((p, o)) => {
                    payload = p;
                    output = o
                }
                Err(s) => status = s,
            },
            (1, 1) => {
                if self.pending.len() >= 256 {
                    reply_into(&mut e.replies, u, -12, &[], 0);
                    return;
                }
                self.pending.push_back(u);
                self.drain(now, false, e);
                return;
            }
            (1, 2) => {
                if let Some(p) = self.device.pop_bulk_in() {
                    payload = p
                } else {
                    if self.pending.len() >= 256 {
                        reply_into(&mut e.replies, u, -12, &[], 0);
                        return;
                    }
                    self.pending.push_back(u);
                    return;
                }
            }
            (0, 1) => output = self.device.interrupt_out(data),
            (0, 2) => output = self.device.bulk_out(data),
            _ => status = -32,
        }
        payload.truncate(u.length);
        let actual = if status != 0 {
            0
        } else if u.direction == 0 {
            data.len()
        } else {
            payload.len()
        };
        reply_into(
            &mut e.replies,
            u,
            status,
            if u.direction == 1 { &payload } else { &[] },
            actual,
        );
        if let Some(o) = output {
            e.outputs.push(o)
        }
        self.drain(now, false, e);
    }
    pub fn unlink(&mut self, u: Urb, e: &mut Effects) {
        let found = self.pending.iter().position(|p| p.seq == u.unlink_seq);
        if let Some(i) = found {
            self.pending.remove(i);
        }
        reply_into(
            &mut e.replies,
            u,
            if found.is_some() { -104 } else { 0 },
            &[],
            0,
        );
    }
    pub fn on_sample(&mut self, sample: InputSample, now: u64, e: &mut Effects) {
        self.sample = sample;
        self.dirty = true;
        self.drain(now, false, e);
    }
    pub fn next_keepalive_at(&self) -> Option<u64> {
        self.pending
            .iter()
            .any(|u| u.ep == 1)
            .then_some(self.last_report.saturating_add(4000))
    }
    pub fn on_keepalive_due(&mut self, now: u64, e: &mut Effects) {
        self.drain(now, true, e);
    }
    fn drain(&mut self, now: u64, keepalive: bool, e: &mut Effects) {
        let mut i = 0;
        while i < self.pending.len() {
            let u = self.pending[i];
            let data = if u.ep == 2 {
                self.device.pop_bulk_in()
            } else if self.dirty || (keepalive && now >= self.last_report.saturating_add(4000)) {
                let p = self.device.interrupt_in(&self.sample);
                if !self.dirty {
                    e.keepalives += 1
                }
                self.dirty = false;
                self.last_report = now;
                e.input_completions += 1;
                Some(p.to_vec())
            } else {
                None
            };
            if let Some(mut p) = data {
                self.pending.remove(i);
                p.truncate(u.length);
                reply_into(&mut e.replies, u, 0, &p, p.len());
            } else {
                i += 1
            }
        }
    }
}
