//! Fixed-capacity in-memory ring buffer for recent snapshots.
//! Matches Netdata's RAM-mode pattern: simple VecDeque + RwLock, zero new deps.
//! Provides 2-second granularity for fast queries without touching SQLite.
//!
//! Each entry pairs an HwSnapshot with a lightweight blame summary so that
//! session_health can be computed entirely from RAM for windows <= 30 min.

use std::collections::VecDeque;
use std::sync::{Arc, RwLock};

use crate::types::{AnomalyType, HwSnapshot, ImpactLevel};

/// Capacity: ~1 hour at 2-second intervals. ~210KB of RAM.
/// Covers the default session_health window (1h) and short hardware_trend
/// queries (last_1h) entirely from RAM, avoiding SQLite I/O.
const DEFAULT_CAPACITY: usize = 1800;

/// Combined snapshot + blame summary stored in the ring buffer.
/// Lightweight: only the fields session_health needs from ProcessBlame.
#[derive(Debug, Clone)]
pub struct RingEntry {
    pub hw: HwSnapshot,
    pub anomaly_type: AnomalyType,
    pub impact_level: ImpactLevel,
    pub anomaly_score: f64,
}

/// Thread-safe ring buffer for recent snapshots + blame summaries.
/// Writer: collector (single, every 2s). Readers: MCP tool handlers (shared locks).
#[derive(Debug, Clone)]
pub struct SnapshotRing {
    inner: Arc<RwLock<VecDeque<RingEntry>>>,
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

    /// Push a new entry. If at capacity, drops the oldest entry.
    pub fn push(&self, entry: RingEntry) {
        let mut buf = self.inner.write().unwrap();
        if buf.len() >= self.capacity {
            buf.pop_front();
        }
        buf.push_back(entry);
    }

    /// Number of entries currently stored.
    pub fn len(&self) -> usize {
        self.inner.read().unwrap().len()
    }

    /// True if no entries stored.
    pub fn is_empty(&self) -> bool {
        self.inner.read().unwrap().is_empty()
    }

    /// Read the most recent HwSnapshot (if any).
    pub fn latest(&self) -> Option<HwSnapshot> {
        self.inner.read().unwrap().back().map(|e| e.hw.clone())
    }

    /// Read all entries within the last `seconds` from the most recent entry.
    pub fn recent(&self, seconds: u64) -> Vec<RingEntry> {
        let buf = self.inner.read().unwrap();
        if buf.is_empty() {
            return Vec::new();
        }
        let latest_ts = buf.back().unwrap().hw.ts;
        let cutoff = latest_ts - chrono::Duration::seconds(seconds as i64);
        buf.iter()
            .filter(|e| e.hw.ts >= cutoff)
            .cloned()
            .collect()
    }

    /// Compute summary statistics over the last N seconds.
    pub fn stats(&self, seconds: u64) -> Option<RingStats> {
        let entries = self.recent(seconds);
        if entries.is_empty() {
            return None;
        }
        let n = entries.len() as f64;
        let mut cpu_sum = 0.0;
        let mut cpu_max = f64::MIN;
        let mut ram_sum = 0.0;
        let mut ram_max = f64::MIN;
        let mut temp_sum = 0.0;
        let mut temp_count = 0;
        let mut temp_max: Option<f64> = None;

        for e in &entries {
            cpu_sum += e.hw.cpu_usage_pct;
            if e.hw.cpu_usage_pct > cpu_max {
                cpu_max = e.hw.cpu_usage_pct;
            }
            ram_sum += e.hw.ram_used_gb;
            if e.hw.ram_used_gb > ram_max {
                ram_max = e.hw.ram_used_gb;
            }
            if let Some(t) = e.hw.die_temp_celsius {
                temp_sum += t;
                temp_count += 1;
                temp_max = Some(temp_max.map_or(t, |m: f64| m.max(t)));
            }
        }

        Some(RingStats {
            sample_count: entries.len(),
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

    /// Compute session health from the ring buffer (no DB for snapshot data).
    /// Uses whatever entries fall within the window. Returns None only if the
    /// ring has zero entries in the window (caller should fall back to DB).
    pub fn session_health(&self, since: chrono::DateTime<chrono::Utc>) -> Option<crate::types::SessionHealth> {
        let buf = self.inner.read().unwrap();
        if buf.is_empty() {
            return None;
        }

        let entries: Vec<&RingEntry> = buf.iter().filter(|e| e.hw.ts >= since).collect();
        if entries.is_empty() {
            return None;
        }

        let n = entries.len() as f64;
        let mut cpu_sum = 0.0_f64;
        let mut cpu_max = 0.0_f64;
        let mut ram_sum = 0.0_f64;
        let mut ram_max = 0.0_f64;
        let mut temp_max: Option<f64> = None;
        let mut score_sum = 0.0_f64;
        let mut throttle_count = 0_u32;
        let mut worst_impact = ImpactLevel::Healthy;
        let mut worst_anomaly = AnomalyType::None;

        for e in &entries {
            cpu_sum += e.hw.cpu_usage_pct;
            if e.hw.cpu_usage_pct > cpu_max { cpu_max = e.hw.cpu_usage_pct; }
            ram_sum += e.hw.ram_used_gb;
            if e.hw.ram_used_gb > ram_max { ram_max = e.hw.ram_used_gb; }
            if let Some(t) = e.hw.die_temp_celsius {
                temp_max = Some(temp_max.map_or(t, |m: f64| m.max(t)));
            }
            score_sum += e.anomaly_score;
            if e.hw.throttling { throttle_count += 1; }
            if impact_rank(&e.impact_level) > impact_rank(&worst_impact) {
                worst_impact = e.impact_level.clone();
            }
            if e.anomaly_type != AnomalyType::None {
                worst_anomaly = e.anomaly_type.clone();
            }
        }

        Some(crate::types::SessionHealth {
            since,
            snapshot_count: entries.len() as u32,
            alert_count: 0, // Ring doesn't track alerts; caller can add from DB if needed
            worst_impact_level: worst_impact,
            worst_anomaly_type: worst_anomaly,
            avg_anomaly_score: score_sum / n,
            avg_cpu_pct: cpu_sum / n,
            avg_ram_gb: ram_sum / n,
            peak_cpu_pct: cpu_max,
            peak_ram_gb: ram_max,
            peak_temp_celsius: temp_max,
            throttle_event_count: throttle_count,
            agent_accumulation_events: entries
                .iter()
                .filter(|e| e.anomaly_type == crate::types::AnomalyType::AgentAccumulation)
                .count() as u32,
            peak_ai_agent_count: entries
                .iter()
                .map(|e| e.hw.ai_agent_count)
                .max()
                .unwrap_or(0),
        })
    }

    /// Compute hardware trend with time-bucketing entirely from the ring.
    /// Returns None if the ring has fewer than 2 entries in the window.
    pub fn hardware_trend(
        &self,
        range_secs: i64,
        bucket_secs: i64,
    ) -> Option<crate::types::TrendData> {
        use crate::types::{TrendBucket, TrendData};

        let buf = self.inner.read().unwrap();
        if buf.len() < 2 {
            return None;
        }

        let now = buf.back().unwrap().hw.ts;
        let cutoff = now - chrono::Duration::seconds(range_secs);
        let entries: Vec<&RingEntry> = buf.iter().filter(|e| e.hw.ts >= cutoff).collect();
        if entries.len() < 2 {
            return None;
        }

        // Bucket entries by time interval
        let first_ts = entries[0].hw.ts;
        let mut buckets: Vec<TrendBucket> = Vec::new();
        let mut bucket_start = first_ts;
        let mut bucket_entries: Vec<&RingEntry> = Vec::new();

        for e in &entries {
            while e.hw.ts >= bucket_start + chrono::Duration::seconds(bucket_secs) {
                if !bucket_entries.is_empty() {
                    buckets.push(compute_bucket(bucket_start, &bucket_entries));
                }
                bucket_entries.clear();
                bucket_start = bucket_start + chrono::Duration::seconds(bucket_secs);
            }
            bucket_entries.push(e);
        }
        // Flush last bucket
        if !bucket_entries.is_empty() {
            buckets.push(compute_bucket(bucket_start, &bucket_entries));
        }

        // Trend direction: compare first-half avg CPU to second-half
        let total_snapshots = entries.len() as u32;
        let trend_direction = if buckets.len() < 2 {
            "insufficient_data".to_string()
        } else {
            let mid = buckets.len() / 2;
            let first_avg: f64 =
                buckets[..mid].iter().map(|b| b.avg_cpu_pct).sum::<f64>() / mid as f64;
            let second_avg: f64 =
                buckets[mid..].iter().map(|b| b.avg_cpu_pct).sum::<f64>() / (buckets.len() - mid) as f64;
            let delta = second_avg - first_avg;
            if delta > 5.0 {
                "rising".to_string()
            } else if delta < -5.0 {
                "falling".to_string()
            } else {
                "stable".to_string()
            }
        };

        Some(TrendData {
            buckets,
            trend_direction,
            total_snapshots,
        })
    }
}

fn compute_bucket(
    bucket_start: chrono::DateTime<chrono::Utc>,
    entries: &[&RingEntry],
) -> crate::types::TrendBucket {
    let n = entries.len() as f64;
    let mut cpu_sum = 0.0_f64;
    let mut cpu_max = 0.0_f64;
    let mut ram_sum = 0.0_f64;
    let mut ram_max = 0.0_f64;
    let mut temp_sum = 0.0_f64;
    let mut temp_count = 0_u32;
    let mut temp_max: Option<f64> = None;
    let mut anomaly_count = 0_u32;
    let mut throttle_count = 0_u32;

    for e in entries {
        cpu_sum += e.hw.cpu_usage_pct;
        if e.hw.cpu_usage_pct > cpu_max {
            cpu_max = e.hw.cpu_usage_pct;
        }
        ram_sum += e.hw.ram_used_gb;
        if e.hw.ram_used_gb > ram_max {
            ram_max = e.hw.ram_used_gb;
        }
        if let Some(t) = e.hw.die_temp_celsius {
            temp_sum += t;
            temp_count += 1;
            temp_max = Some(temp_max.map_or(t, |m: f64| m.max(t)));
        }
        if e.anomaly_type != AnomalyType::None {
            anomaly_count += 1;
        }
        if e.hw.throttling {
            throttle_count += 1;
        }
    }

    crate::types::TrendBucket {
        bucket_start,
        sample_count: entries.len() as u32,
        avg_cpu_pct: cpu_sum / n,
        peak_cpu_pct: cpu_max,
        avg_ram_gb: ram_sum / n,
        peak_ram_gb: ram_max,
        avg_temp_celsius: if temp_count > 0 {
            Some(temp_sum / temp_count as f64)
        } else {
            None
        },
        peak_temp_celsius: temp_max,
        anomaly_count,
        throttle_count,
    }
}

fn impact_rank(level: &ImpactLevel) -> u8 {
    match level {
        ImpactLevel::Healthy => 0,
        ImpactLevel::Degrading => 1,
        ImpactLevel::Strained => 2,
        ImpactLevel::Critical => 3,
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

    fn make_entry(cpu: f64, ram: f64, temp: Option<f64>) -> RingEntry {
        RingEntry {
            hw: HwSnapshot {
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
                cpu_trend: TrendDirection::Stable,
                ram_trend: TrendDirection::Stable,
                temp_trend: TrendDirection::Stable,
                cpu_delta_pct: 0.0,
                ram_delta_gb: 0.0,
                top_culprit: String::new(),
                impact_level: ImpactLevel::Healthy,
                impact_duration_s: 0,
                one_liner: String::new(),
                ai_agent_count: 0,
                ai_agent_ram_gb: 0.0,
                swap_used_gb: None,
                swap_total_gb: None,
                disk_fill_rate_gb_per_sec: None,
                irq_per_sec: None,
            },
            anomaly_type: AnomalyType::None,
            impact_level: ImpactLevel::Healthy,
            anomaly_score: 0.0,
        }
    }

    fn make_entry_at(cpu: f64, ram: f64, ts: chrono::DateTime<Utc>) -> RingEntry {
        RingEntry {
            hw: HwSnapshot {
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
                cpu_trend: TrendDirection::Stable,
                ram_trend: TrendDirection::Stable,
                temp_trend: TrendDirection::Stable,
                cpu_delta_pct: 0.0,
                ram_delta_gb: 0.0,
                top_culprit: String::new(),
                impact_level: ImpactLevel::Healthy,
                impact_duration_s: 0,
                one_liner: String::new(),
                ai_agent_count: 0,
                ai_agent_ram_gb: 0.0,
                swap_used_gb: None,
                swap_total_gb: None,
                disk_fill_rate_gb_per_sec: None,
                irq_per_sec: None,
            },
            anomaly_type: AnomalyType::None,
            impact_level: ImpactLevel::Healthy,
            anomaly_score: 0.0,
        }
    }

    #[test]
    fn test_push_and_len() {
        let ring = SnapshotRing::with_capacity(3);
        assert!(ring.is_empty());
        ring.push(make_entry(10.0, 2.0, None));
        assert_eq!(ring.len(), 1);
        ring.push(make_entry(20.0, 3.0, None));
        ring.push(make_entry(30.0, 4.0, None));
        assert_eq!(ring.len(), 3);
    }

    #[test]
    fn test_capacity_eviction() {
        let ring = SnapshotRing::with_capacity(2);
        ring.push(make_entry(10.0, 1.0, None));
        ring.push(make_entry(20.0, 2.0, None));
        ring.push(make_entry(30.0, 3.0, None));
        assert_eq!(ring.len(), 2);
        // Oldest (10.0) should be evicted
        let latest = ring.latest().unwrap();
        assert!((latest.cpu_usage_pct - 30.0).abs() < 0.01);
    }

    #[test]
    fn test_latest() {
        let ring = SnapshotRing::new();
        assert!(ring.latest().is_none());
        ring.push(make_entry(10.0, 1.0, None));
        ring.push(make_entry(50.0, 3.0, None));
        let latest = ring.latest().unwrap();
        assert!((latest.cpu_usage_pct - 50.0).abs() < 0.01);
    }

    #[test]
    fn test_recent_time_window() {
        let ring = SnapshotRing::new();
        let now = Utc::now();
        ring.push(make_entry_at(10.0, 1.0, now - chrono::Duration::seconds(60)));
        ring.push(make_entry_at(20.0, 2.0, now - chrono::Duration::seconds(30)));
        ring.push(make_entry_at(30.0, 3.0, now - chrono::Duration::seconds(10)));
        ring.push(make_entry_at(40.0, 4.0, now - chrono::Duration::seconds(2)));

        let recent = ring.recent(15); // last 15 seconds
        assert_eq!(recent.len(), 2); // -10s and -2s
        assert!((recent[0].hw.cpu_usage_pct - 30.0).abs() < 0.01);
        assert!((recent[1].hw.cpu_usage_pct - 40.0).abs() < 0.01);
    }

    #[test]
    fn test_stats_computation() {
        let ring = SnapshotRing::new();
        ring.push(make_entry(10.0, 2.0, Some(60.0)));
        ring.push(make_entry(30.0, 4.0, Some(80.0)));
        ring.push(make_entry(20.0, 3.0, None));

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
        ring.push(make_entry(50.0, 2.0, None));
        // Both handles see the same data (Arc)
        assert_eq!(ring2.len(), 1);
    }

    #[test]
    fn test_session_health_from_ring() {
        let ring = SnapshotRing::new();
        let now = Utc::now();
        let mut e1 = make_entry_at(20.0, 8.0, now - chrono::Duration::seconds(120));
        e1.impact_level = ImpactLevel::Degrading;
        e1.anomaly_score = 0.3;
        ring.push(e1);

        let mut e2 = make_entry_at(80.0, 12.0, now - chrono::Duration::seconds(60));
        e2.impact_level = ImpactLevel::Strained;
        e2.anomaly_score = 0.7;
        e2.hw.throttling = true;
        ring.push(e2);

        let mut e3 = make_entry_at(15.0, 7.0, now - chrono::Duration::seconds(2));
        e3.impact_level = ImpactLevel::Healthy;
        e3.anomaly_score = 0.1;
        ring.push(e3);

        // Query last 5 minutes — ring covers it
        let since = now - chrono::Duration::seconds(300);
        let health = ring.session_health(since).expect("ring should cover 5 min");
        assert_eq!(health.snapshot_count, 3);
        assert!((health.peak_cpu_pct - 80.0).abs() < 0.01);
        assert!((health.peak_ram_gb - 12.0).abs() < 0.01);
        assert_eq!(health.throttle_event_count, 1);
        assert!(matches!(health.worst_impact_level, ImpactLevel::Strained));
    }
}
