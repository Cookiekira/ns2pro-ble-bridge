//! Bounded, allocation-free latency samples. Measured receipt time is not BLE interval.
use std::time::{Duration, Instant};
#[derive(Debug)]
pub struct Statistics {
    connection: crate::ble::ConnectionReport,
    start: Instant,
    last: Option<Instant>,
    reports: u64,
    urbs: u64,
    keepalives: u64,
    rumble_writes: u64,
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
            connection: Default::default(),
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
    pub fn connected(&mut self, report: crate::ble::ConnectionReport) {
        self.connection = report;
    }
    pub fn disconnected(&mut self) {
        self.connection = Default::default();
    }
    pub fn connection_report(&self) -> crate::ble::ConnectionReport {
        self.connection
    }
    pub fn rumble_written(&mut self) {
        self.rumble_writes += 1;
    }
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
            target_interval = ?self.connection.target,
            preference_result = ?self.connection.preference,
            observed_interval_ms = %self
                .connection
                .observed_interval
                .map(|units| (f64::from(units) * 1.25).to_string())
                .unwrap_or_else(|| "unknown".into()),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ble::{ConnectionReport, ConnectionTarget, PreferenceOutcome};

    #[test]
    fn connection_events_preserve_cumulative_measurements() {
        let mut stats = Statistics::default();
        let report = ConnectionReport {
            target: ConnectionTarget::Interval(6),
            preference: PreferenceOutcome::Confirmed,
            observed_interval: Some(6),
        };
        let start = stats.start;
        assert_eq!(stats.connection_report(), ConnectionReport::default());
        stats.connected(report);
        assert_eq!(stats.connection_report(), report);
        stats.receipt(1);
        stats.completion(1, 3, 2);
        stats.rumble_written();
        stats.rumble_written();
        let last = stats.last;
        stats.disconnected();
        assert_eq!(stats.connection_report(), ConnectionReport::default());
        stats.connected(report);
        assert_eq!(stats.connection_report(), report);
        assert_eq!(stats.start, start);
        assert_eq!(stats.last, last);
        assert_eq!(stats.reports, 1);
        assert_eq!(stats.urbs, 3);
        assert_eq!(stats.keepalives, 2);
        assert_eq!(stats.rumble_writes, 2);
        assert_eq!(stats.len, 1);
    }

    #[test]
    fn receipts_measure_interarrival_and_replace_pending_sample() {
        let mut stats = Statistics::default();
        stats.receipt(1);
        let first = stats.last.unwrap();
        assert_eq!(stats.interval_sum, 0);
        stats.receipt(2);
        let interval = stats.last.unwrap().duration_since(first).as_micros() as u64;
        assert_eq!(stats.reports, 2);
        assert_eq!(stats.interval_sum, u128::from(interval));
        assert_eq!(stats.interval_min, interval);
        assert_eq!(stats.interval_max, interval);
        stats.completion(1, 1, 0);
        assert_eq!(stats.len, 0);
        stats.completion(2, 1, 0);
        assert_eq!(stats.len, 1);
    }

    #[test]
    fn only_matching_fresh_input_records_latency_once() {
        let mut stats = Statistics::default();
        stats.completion(7, 1, 0);
        assert_eq!(stats.len, 0);
        stats.receipt(7);
        stats.completion(7, 0, 0);
        stats.completion(7, 2, 2);
        stats.completion(8, 1, 0);
        assert_eq!(stats.len, 0);
        stats.disconnected();
        stats.completion(7, 3, 1);
        assert_eq!(stats.len, 1);
        stats.completion(7, 1, 0);
        assert_eq!(stats.len, 1);
        assert_eq!(stats.urbs, 8);
        assert_eq!(stats.keepalives, 3);
    }

    #[test]
    fn latency_samples_remain_bounded_while_counters_accumulate() {
        let mut stats = Statistics::default();
        for seq in 0..4097 {
            stats.receipt(seq);
            stats.completion(seq, 1, 0);
        }
        assert_eq!(stats.len, 4096);
        assert_eq!(stats.cursor, 1);
        assert_eq!(stats.reports, 4097);
        assert_eq!(stats.urbs, 4097);
        assert_eq!(stats.keepalives, 0);
    }
}
