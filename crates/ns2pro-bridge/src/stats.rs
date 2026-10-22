//! Bounded, allocation-free latency samples. Measured receipt time is not BLE interval.
use std::time::{Duration, Instant};
#[derive(Debug)]
pub struct Statistics {
    start: Instant,
    last: Option<Instant>,
    pub reports: u64,
    pub urbs: u64,
    pub keepalives: u64,
    pub rumble_writes: u64,
    interval_sum: u128,
    interval_min: u64,
    interval_max: u64,
    arrival: Option<(u64, Instant)>,
    latencies: [u64; 4096],
    len: usize,
    cursor: usize,
}
impl Default for Statistics {
    fn default() -> Self {
        Self {
            start: Instant::now(),
            last: None,
            reports: 0,
            urbs: 0,
            keepalives: 0,
            rumble_writes: 0,
            interval_sum: 0,
            interval_min: u64::MAX,
            interval_max: 0,
            arrival: None,
            latencies: [0; 4096],
            len: 0,
            cursor: 0,
        }
    }
}
impl Statistics {
    pub fn receipt(&mut self, seq: u64) {
        let now = Instant::now();
        if let Some(last) = self.last {
            let us = now.duration_since(last).as_micros() as u64;
            self.interval_sum += u128::from(us);
            self.interval_min = self.interval_min.min(us);
            self.interval_max = self.interval_max.max(us);
        }
        self.last = Some(now);
        self.arrival = Some((seq, now));
        self.reports += 1;
    }
    pub fn completion(&mut self, seq: u64, inputs: u64, keepalives: u64) {
        self.urbs += inputs;
        self.keepalives += keepalives;
        if inputs > keepalives
            && let Some((s, t)) = self.arrival
            && s == seq
        {
            self.latencies[self.cursor] = t.elapsed().as_nanos() as u64;
            self.cursor = (self.cursor + 1) % 4096;
            self.len = (self.len + 1).min(4096);
            self.arrival = None;
        }
    }
    pub fn log(&self) {
        let mut values = self.latencies;
        values[..self.len].sort_unstable();
        let p = |percent: usize| {
            if self.len == 0 {
                0
            } else {
                values[(self.len - 1) * percent / 100]
            }
        };
        let secs = self.start.elapsed().as_secs_f64().max(0.001);
        tracing::debug!(
            ble_hz = self.reports as f64 / secs,
            interarrival_min_us = if self.reports > 1 {
                self.interval_min
            } else {
                0
            },
            interarrival_max_us = self.interval_max,
            interarrival_mean_us = if self.reports > 1 {
                self.interval_sum / (self.reports - 1) as u128
            } else {
                0
            },
            bridge_p50_ns = p(50),
            bridge_p99_ns = p(99),
            urb_hz = self.urbs as f64 / secs,
            keepalives = self.keepalives,
            rumble_hz = self.rumble_writes as f64 / secs,
            observed_interval = "unknown",
            "bridge statistics (receipt to completed socket write; excludes BLE and host)"
        );
    }
}
pub async fn diagnostics(
    channels: crate::channels::Channels,
    cancel: tokio_util::sync::CancellationToken,
) {
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    loop {
        tokio::select! {
            _ = cancel.cancelled() => return,
            _ = tick.tick() => channels.stats.lock().unwrap().log(),
        }
    }
}
