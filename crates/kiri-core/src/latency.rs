//! Latency distribution accumulator (specs/TRACE.md, T003 acceptance).
//!
//! Records per-request execution durations and summarizes them as a stable
//! distribution. Used by the control-plane benchmark to emit p50/p99 without
//! pulling in a statistics crate.

use serde::Serialize;

/// Accumulated latency samples in nanoseconds.
#[derive(Debug, Default, Clone)]
pub struct LatencyDistribution {
    samples: Vec<u64>,
    sum_ns: u64,
    max_ns: u64,
    min_ns: u64,
    count: u64,
}

impl LatencyDistribution {
    pub fn new() -> Self {
        Self::default()
    }

    /// Record one observed duration in nanoseconds.
    pub fn record(&mut self, ns: u64) {
        self.samples.push(ns);
        self.sum_ns = self.sum_ns.saturating_add(ns);
        self.max_ns = self.max_ns.max(ns);
        if self.count == 0 {
            self.min_ns = ns;
        } else {
            self.min_ns = self.min_ns.min(ns);
        }
        self.count += 1;
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    pub fn min_ns(&self) -> u64 {
        self.min_ns
    }

    pub fn max_ns(&self) -> u64 {
        self.max_ns
    }

    pub fn mean_ns(&self) -> u64 {
        self.sum_ns.checked_div(self.count).unwrap_or(0)
    }

    /// Percentile (0..=100) of recorded samples, nearest-rank method.
    pub fn percentile_ns(&self, p: u8) -> u64 {
        if self.samples.is_empty() {
            return 0;
        }
        let mut sorted = self.samples.clone();
        sorted.sort_unstable();
        let p = p.clamp(0, 100) as u64;
        // nearest-rank: index = ceil(p/100 * n) - 1
        let n = sorted.len() as u64;
        let rank = (p * n).div_ceil(100).saturating_sub(1) as usize;
        sorted[rank.min(sorted.len() - 1)]
    }

    /// Reset to empty, retaining allocated capacity.
    pub fn clear(&mut self) {
        self.samples.clear();
        self.sum_ns = 0;
        self.max_ns = 0;
        self.min_ns = 0;
        self.count = 0;
    }

    /// A serializable summary for the benchmark report.
    pub fn summary(&self) -> LatencySummary {
        LatencySummary {
            schema_version: 1,
            count: self.count,
            min_ns: self.min_ns,
            mean_ns: self.mean_ns(),
            p50_ns: self.percentile_ns(50),
            p99_ns: self.percentile_ns(99),
            max_ns: self.max_ns,
        }
    }
}

/// Serializable latency summary (schema_version 1).
#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct LatencySummary {
    pub schema_version: u32,
    pub count: u64,
    pub min_ns: u64,
    pub mean_ns: u64,
    pub p50_ns: u64,
    pub p99_ns: u64,
    pub max_ns: u64,
}

impl Default for LatencySummary {
    fn default() -> Self {
        LatencySummary {
            schema_version: 1,
            count: 0,
            min_ns: 0,
            mean_ns: 0,
            p50_ns: 0,
            p99_ns: 0,
            max_ns: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_distribution_is_zero() {
        let d = LatencyDistribution::new();
        assert!(d.is_empty());
        assert_eq!(d.summary().count, 0);
        assert_eq!(d.percentile_ns(99), 0);
    }

    #[test]
    fn records_min_max_mean_and_percentiles() {
        let mut d = LatencyDistribution::new();
        // 1..=100 ns, monotonic
        for i in 1u64..=100 {
            d.record(i);
        }
        assert_eq!(d.count(), 100);
        assert_eq!(d.min_ns(), 1);
        assert_eq!(d.max_ns(), 100);
        assert_eq!(d.mean_ns(), 50);
        // p50 of 1..=100 nearest-rank ~ 50
        assert_eq!(d.percentile_ns(50), 50);
        // p99 of 1..=100 nearest-rank ~ 99
        assert_eq!(d.percentile_ns(99), 99);
    }

    #[test]
    fn summary_serializes_to_schema_shape() {
        let mut d = LatencyDistribution::new();
        d.record(10);
        d.record(20);
        let json = serde_json::to_value(&d.summary()).unwrap();
        assert_eq!(json["schema_version"], 1);
        assert_eq!(json["count"], 2);
        assert_eq!(json["min_ns"], 10);
        assert_eq!(json["max_ns"], 20);
    }

    #[test]
    fn clear_resets_state() {
        let mut d = LatencyDistribution::new();
        d.record(5);
        d.clear();
        assert!(d.is_empty());
        assert_eq!(d.count(), 0);
    }
}
