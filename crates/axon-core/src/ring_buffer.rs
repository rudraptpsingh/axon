//! Fixed-capacity in-memory ring buffer for recent HwSnapshots.
//! Matches Netdata's RAM-mode pattern: simple VecDeque + RwLock, zero new deps.
//! Provides 2-second granularity for fast trend queries without touching SQLite.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use crate::types::HwSnapshot;

/// Capacity: ~30 minutes at 2-second intervals. ~72KB of RAM.
const DEFAULT_CAPACITY: usize = 900;

/// Thread-safe ring buffer for recent hardware snapshots.
/// Writer: collector (single, every 2s). Readers: MCP tool handlers (shared locks).
#[derive(Debug, Clone)]
pub struct SnapshotRing {
    inner: Arc<RwLock<VecDeque<HwSnapshot>>>,
    capacity: usize,
}

impl SnapshotRing {
    pub fn new() -> Self {
        Self::with_capacity(DEFAULT_CAPACITY)
    }

    pub fn with_capacity(capacity: usize) -> Self {
        Self {
            inner: Arc::new(RwLock::new(VecDeque::with_capacity(capacity))),
            capacity,
        }
    }

    /// Push a new snapshot. If at capacity, drops the oldest entry.
    pub fn push(&self, snapshot: HwSnapshot) {
        let mut buf = self.inner.write().unwrap();
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(snapshot);
    }

    /// Number of snapshots currently stored.
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    /// True if no snapshots stored.
    pub fn is_empty(&self) -> bool {
        self.inner.read().unwrap().is_empty()
    }

    /// Read the most recent snapshot (if any).
    pub fn latest(&self) -> Option<HwSnapshot> {
        self.inner.read().unwrap().back().cloned()
    }

    /// Read all snapshots within the last `seconds` from the most recent entry.
    pub fn recent(&self, seconds: u64) -> Vec<HwSnapshot> {
        let buf = self.inner.read().unwrap();
        if buf.is_empty() {
            return Vec::new();
        }
        let latest_ts = buf.back().unwrap().ts;
        let cutoff = latest_ts - chrono::Duration::seconds(seconds as i64);
        buf.iter().filter(|s| s.ts >= cutoff).cloned().collect()
    }

    /// Compute summary statistics over the last N seconds.
    pub fn stats(&self, seconds: u64) -> Option<RingStats> {
        let snapshots = self.recent(seconds);
        if snapshots.is_empty() {
            return None;
        }
        let n = snapshots.len() as f64;
        let mut cpu_sum = 0.0;
        let mut cpu_max = f64::MIN;
        let mut ram_sum = 0.0;
        let mut ram_max = f64::MIN;
        let mut temp_sum = 0.0;
        let mut temp_count = 0;
        let mut temp_max: Option<f64> = None;

        for s in &snapshots {
            cpu_sum += s.cpu_usage_pct;
            if s.cpu_usage_pct > cpu_max {
                cpu_max = s.cpu_usage_pct;
            }
            ram_sum += s.ram_used_gb;
            if s.ram_used_gb > ram_max {
                ram_max = s.ram_used_gb;
            }
            if let Some(t) = s.die_temp_celsius {
                temp_sum += t;
                temp_count += 1;
                temp_max = Some(temp_max.map_or(t, |m: f64| m.max(t)));
            }
        }

        Some(RingStats {
            sample_count: snapshots.len(),
            cpu_avg: cpu_sum / n,
            cpu_max,
            ram_avg_gb: ram_sum / n,
            ram_max_gb: ram_max,
            temp_avg: if temp_count > 0 {
                Some(temp_sum / temp_count as f64)
            } else {
                None
            },
            temp_max,
        })
    }
}

impl Default for SnapshotRing {
    fn default() -> Self {
        Self::new()
    }
}

/// Summary statistics from the ring buffer.
#[derive(Debug, Clone)]
pub struct RingStats {
    pub sample_count: usize,
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub ram_avg_gb: f64,
    pub ram_max_gb: f64,
    pub temp_avg: Option<f64>,
    pub temp_max: Option<f64>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;
    use chrono::Utc;

    fn make_snapshot(cpu: f64, ram: f64, temp: Option<f64>) -> HwSnapshot {
        HwSnapshot {
            die_temp_celsius: temp,
            throttling: false,
            ram_used_gb: ram,
            ram_total_gb: 16.0,
            ram_pressure: RamPressure::Normal,
            cpu_usage_pct: cpu,
            disk_used_gb: 100.0,
            disk_total_gb: 500.0,
            disk_pressure: DiskPressure::Normal,
            headroom: HeadroomLevel::Adequate,
            headroom_reason: "ok".to_string(),
            ts: Utc::now(),
        }
    }

    fn make_snapshot_at(cpu: f64, ram: f64, ts: chrono::DateTime<Utc>) -> HwSnapshot {
        HwSnapshot {
            die_temp_celsius: None,
            throttling: false,
            ram_used_gb: ram,
            ram_total_gb: 16.0,
            ram_pressure: RamPressure::Normal,
            cpu_usage_pct: cpu,
            disk_used_gb: 100.0,
            disk_total_gb: 500.0,
            disk_pressure: DiskPressure::Normal,
            headroom: HeadroomLevel::Adequate,
            headroom_reason: "ok".to_string(),
            ts,
        }
    }

    #[test]
    fn test_push_and_len() {
        let ring = SnapshotRing::with_capacity(3);
        assert!(ring.is_empty());
        ring.push(make_snapshot(10.0, 2.0, None));
        assert_eq!(ring.len(), 1);
        ring.push(make_snapshot(20.0, 3.0, None));
        ring.push(make_snapshot(30.0, 4.0, None));
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn test_capacity_eviction() {
        let ring = SnapshotRing::with_capacity(2);
        ring.push(make_snapshot(10.0, 1.0, None));
        ring.push(make_snapshot(20.0, 2.0, None));
        ring.push(make_snapshot(30.0, 3.0, None));
        assert_eq!(ring.len(), 2);
        // Oldest (10.0) should be evicted
        let latest = ring.latest().unwrap();
        assert!((latest.cpu_usage_pct - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_latest() {
        let ring = SnapshotRing::new();
        assert!(ring.latest().is_none());
        ring.push(make_snapshot(10.0, 1.0, None));
        ring.push(make_snapshot(50.0, 3.0, None));
        let latest = ring.latest().unwrap();
        assert!((latest.cpu_usage_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_recent_time_window() {
        let ring = SnapshotRing::new();
        let now = Utc::now();
        // Push snapshots at -60s, -30s, -10s, -2s
        ring.push(make_snapshot_at(
            10.0,
            1.0,
            now - chrono::Duration::seconds(60),
        ));
        ring.push(make_snapshot_at(
            20.0,
            2.0,
            now - chrono::Duration::seconds(30),
        ));
        ring.push(make_snapshot_at(
            30.0,
            3.0,
            now - chrono::Duration::seconds(10),
        ));
        ring.push(make_snapshot_at(
            40.0,
            4.0,
            now - chrono::Duration::seconds(2),
        ));

        let recent = ring.recent(15); // last 15 seconds
        assert_eq!(recent.len(), 2); // -10s and -2s
        assert!((recent[0].cpu_usage_pct - 30.0).abs() < 0.01);
        assert!((recent[1].cpu_usage_pct - 40.0).abs() < 0.01);
    }

    #[test]
    fn test_stats_computation() {
        let ring = SnapshotRing::new();
        ring.push(make_snapshot(10.0, 2.0, Some(60.0)));
        ring.push(make_snapshot(30.0, 4.0, Some(80.0)));
        ring.push(make_snapshot(20.0, 3.0, None));

        let stats = ring.stats(3600).unwrap(); // last hour
        assert_eq!(stats.sample_count, 3);
        assert!((stats.cpu_avg - 20.0).abs() < 0.01);
        assert!((stats.cpu_max - 30.0).abs() < 0.01);
        assert!((stats.ram_avg_gb - 3.0).abs() < 0.01);
        assert!((stats.ram_max_gb - 4.0).abs() < 0.01);
        assert!((stats.temp_avg.unwrap() - 70.0).abs() < 0.01);
        assert!((stats.temp_max.unwrap() - 80.0).abs() < 0.01);
    }

    #[test]
    fn test_stats_empty_returns_none() {
        let ring = SnapshotRing::new();
        assert!(ring.stats(60).is_none());
    }

    #[test]
    fn test_thread_safety_clone() {
        let ring = SnapshotRing::new();
        let ring2 = ring.clone();
        ring.push(make_snapshot(50.0, 2.0, None));
        // Both handles see the same data (Arc)
        assert_eq!(ring2.len(), 1);
    }
}
