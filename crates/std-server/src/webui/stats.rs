//! Atomic request statistics surfaced by the admin panel.

use std::sync::atomic::{AtomicU64, Ordering};

/// Cumulative server statistics, updated once per served response. All counters
/// are monotonic since process start and use relaxed ordering (independent
/// counters, no cross-counter invariant).
#[derive(Default)]
pub struct Stats {
    total: AtomicU64,
    status_2xx: AtomicU64,
    status_3xx: AtomicU64,
    status_4xx: AtomicU64,
    status_5xx: AtomicU64,
    bytes_out: AtomicU64,
}

impl Stats {
    pub fn new() -> Stats {
        Stats::default()
    }

    /// Records one served response by its status code and body length.
    pub fn record(&self, status: u16, body_len: usize) {
        self.total.fetch_add(1, Ordering::Relaxed);
        let bucket = match status {
            200..=299 => &self.status_2xx,
            300..=399 => &self.status_3xx,
            400..=499 => &self.status_4xx,
            _ => &self.status_5xx,
        };
        bucket.fetch_add(1, Ordering::Relaxed);
        self.bytes_out.fetch_add(body_len as u64, Ordering::Relaxed);
    }

    /// Serializes a snapshot as JSON, including the caller-provided uptime.
    pub fn snapshot_json(&self, uptime_secs: u64) -> String {
        format!(
            "{{\"uptime_secs\":{},\"total_requests\":{},\"status_2xx\":{},\"status_3xx\":{},\"status_4xx\":{},\"status_5xx\":{},\"bytes_out\":{}}}",
            uptime_secs,
            self.total.load(Ordering::Relaxed),
            self.status_2xx.load(Ordering::Relaxed),
            self.status_3xx.load(Ordering::Relaxed),
            self.status_4xx.load(Ordering::Relaxed),
            self.status_5xx.load(Ordering::Relaxed),
            self.bytes_out.load(Ordering::Relaxed),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_into_status_buckets_and_totals() {
        // Each response must land in exactly one status bucket and add to the
        // totals, so the panel's numbers reflect real traffic.
        let stats = Stats::new();
        stats.record(200, 100);
        stats.record(204, 0);
        stats.record(404, 50);
        stats.record(500, 10);
        let json = stats.snapshot_json(42);
        assert!(json.contains("\"total_requests\":4"), "{json}");
        assert!(json.contains("\"status_2xx\":2"), "{json}");
        assert!(json.contains("\"status_4xx\":1"), "{json}");
        assert!(json.contains("\"status_5xx\":1"), "{json}");
        assert!(json.contains("\"bytes_out\":160"), "{json}");
        assert!(json.contains("\"uptime_secs\":42"), "{json}");
    }
}
