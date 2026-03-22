use std::collections::HashMap;

/// Smoothing factor for medium-term baseline (original): α=0.2, ~5-sample window (~10s at 2s ticks).
const ALPHA_MEDIUM: f64 = 0.2;
/// Smoothing factor for fast baseline: α=0.4, ~2.5-sample window (~5s). Catches spikes quickly.
const ALPHA_FAST: f64 = 0.4;
/// Smoothing factor for slow baseline: α=0.05, ~20-sample window (~40s). Catches gradual drifts.
const ALPHA_SLOW: f64 = 0.05;

/// Minimum samples before each timescale reports deltas.
const WARMUP_FAST: u32 = 2;
const WARMUP_MEDIUM: u32 = 3;
const WARMUP_SLOW: u32 = 8;

/// Single-timescale EWMA tracker.
#[derive(Debug, Clone)]
struct EwmaTracker {
    cpu: f64,
    ram: f64,
    alpha: f64,
    initialized: bool,
}

impl EwmaTracker {
    fn new(alpha: f64) -> Self {
        Self {
            cpu: 0.0,
            ram: 0.0,
            alpha,
            initialized: false,
        }
    }

    fn update(&mut self, cpu: f64, ram: f64) {
        if !self.initialized {
            self.cpu = cpu;
            self.ram = ram;
            self.initialized = true;
        } else {
            self.cpu = self.alpha * cpu + (1.0 - self.alpha) * self.cpu;
            self.ram = self.alpha * ram + (1.0 - self.alpha) * self.ram;
        }
    }
}

/// Per-process multi-timescale EWMA baseline tracker.
/// Tracks fast (spike detection), medium (blame scoring), and slow (drift detection) baselines.
#[derive(Debug, Clone)]
pub struct ProcessBaseline {
    fast: EwmaTracker,
    medium: EwmaTracker,
    slow: EwmaTracker,
    pub samples: u32,

    // Public accessors for backward compatibility
    pub cpu_ewma: f64,
    pub ram_ewma: f64,
}

impl ProcessBaseline {
    fn new() -> Self {
        Self {
            fast: EwmaTracker::new(ALPHA_FAST),
            medium: EwmaTracker::new(ALPHA_MEDIUM),
            slow: EwmaTracker::new(ALPHA_SLOW),
            samples: 0,
            cpu_ewma: 0.0,
            ram_ewma: 0.0,
        }
    }

    /// Update all three timescale baselines with a new observation.
    /// Returns (cpu_delta, ram_delta) using the medium-timescale EWMA (backward compatible).
    pub fn update(&mut self, cpu: f64, ram: f64) -> (f64, f64) {
        self.fast.update(cpu, ram);
        self.medium.update(cpu, ram);
        self.slow.update(cpu, ram);
        self.samples += 1;

        // Keep backward-compatible fields in sync with medium
        self.cpu_ewma = self.medium.cpu;
        self.ram_ewma = self.medium.ram;

        // Only report delta after medium warmup (3 samples)
        if self.samples < WARMUP_MEDIUM {
            return (0.0, 0.0);
        }
        let cpu_delta = (cpu - self.medium.cpu).max(0.0);
        let ram_delta = (ram - self.medium.ram).max(0.0);
        (cpu_delta, ram_delta)
    }

    /// Returns fast-timescale deltas (spike detection). Positive only.
    /// Available after WARMUP_FAST samples.
    pub fn fast_delta(&self, cpu: f64, ram: f64) -> (f64, f64) {
        if self.samples < WARMUP_FAST {
            return (0.0, 0.0);
        }
        ((cpu - self.fast.cpu).max(0.0), (ram - self.fast.ram).max(0.0))
    }

    /// Returns slow-timescale deltas (drift detection). Positive only.
    /// Available after WARMUP_SLOW samples.
    pub fn slow_delta(&self, cpu: f64, ram: f64) -> (f64, f64) {
        if self.samples < WARMUP_SLOW {
            return (0.0, 0.0);
        }
        ((cpu - self.slow.cpu).max(0.0), (ram - self.slow.ram).max(0.0))
    }

    /// True when the slow baseline diverges significantly from the fast baseline,
    /// indicating a gradual drift (e.g., memory leak). Requires slow warmup.
    pub fn drift_detected(&self) -> bool {
        if self.samples < WARMUP_SLOW {
            return false;
        }
        // Drift: slow baseline is notably lower than fast baseline (resource usage
        // has been climbing gradually). A >20% CPU spread or >0.5GB RAM spread signals drift.
        let cpu_spread = self.fast.cpu - self.slow.cpu;
        let ram_spread = self.fast.ram - self.slow.ram;
        cpu_spread > 20.0 || ram_spread > 0.5
    }
}

/// Stores per-process EWMA baselines, keyed by PID.
#[derive(Debug, Default)]
pub struct EwmaStore {
    baselines: HashMap<u32, ProcessBaseline>,
}

impl EwmaStore {
    /// Update baseline for a process and return (cpu_delta, ram_delta) from medium timescale.
    pub fn update(&mut self, pid: u32, cpu: f64, ram: f64) -> (f64, f64) {
        let baseline = self
            .baselines
            .entry(pid)
            .or_insert_with(ProcessBaseline::new);
        baseline.update(cpu, ram)
    }

    /// Get the baseline for a given PID (if it exists).
    pub fn get(&self, pid: u32) -> Option<&ProcessBaseline> {
        self.baselines.get(&pid)
    }

    /// Remove baselines for PIDs that are no longer active (avoids unbounded growth).
    pub fn cleanup(&mut self, active_pids: &[u32]) {
        self.baselines.retain(|pid, _| active_pids.contains(pid));
    }

    #[cfg(test)]
    pub fn len(&self) -> usize {
        self.baselines.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_first_sample_returns_zero_delta() {
        let mut store = EwmaStore::default();
        let (cpu_d, ram_d) = store.update(100, 50.0, 2.0);
        assert_eq!(cpu_d, 0.0);
        assert_eq!(ram_d, 0.0);
    }

    #[test]
    fn test_deltas_zero_until_three_samples() {
        let mut store = EwmaStore::default();
        // Sample 1: init
        let (c, r) = store.update(1, 50.0, 2.0);
        assert_eq!((c, r), (0.0, 0.0));
        // Sample 2: still warming up (samples < 3)
        let (c, r) = store.update(1, 50.0, 2.0);
        assert_eq!((c, r), (0.0, 0.0));
        // Sample 3: now reports delta (stable values = near-zero delta)
        let (c, r) = store.update(1, 50.0, 2.0);
        assert!(c < 1.0, "stable CPU should produce near-zero delta");
        assert!(r < 0.1, "stable RAM should produce near-zero delta");
    }

    #[test]
    fn test_spike_produces_positive_delta() {
        let mut store = EwmaStore::default();
        // Build baseline with 5 stable samples
        for _ in 0..5 {
            store.update(1, 10.0, 1.0);
        }
        // Spike
        let (cpu_d, ram_d) = store.update(1, 90.0, 5.0);
        assert!(
            cpu_d > 40.0,
            "CPU spike should produce large delta, got {}",
            cpu_d
        );
        assert!(
            ram_d > 2.0,
            "RAM spike should produce large delta, got {}",
            ram_d
        );
    }

    #[test]
    fn test_negative_delta_clamped_to_zero() {
        let mut store = EwmaStore::default();
        // Build baseline at high usage
        for _ in 0..5 {
            store.update(1, 80.0, 4.0);
        }
        // Drop to low usage
        let (cpu_d, ram_d) = store.update(1, 10.0, 0.5);
        assert_eq!(cpu_d, 0.0, "negative CPU delta should be clamped to 0");
        assert_eq!(ram_d, 0.0, "negative RAM delta should be clamped to 0");
    }

    #[test]
    fn test_cleanup_removes_stale_pids() {
        let mut store = EwmaStore::default();
        store.update(1, 10.0, 1.0);
        store.update(2, 20.0, 2.0);
        store.update(3, 30.0, 3.0);
        assert_eq!(store.len(), 3);

        store.cleanup(&[1, 3]);
        assert_eq!(store.len(), 2);

        // PID 2 should be gone; PIDs 1 and 3 should remain
        let (c, _) = store.update(1, 10.0, 1.0);
        assert_eq!(c, 0.0); // second sample, still warming

        // PID 2 should be re-initialized
        let (c, _) = store.update(2, 20.0, 2.0);
        assert_eq!(c, 0.0); // first sample again
        assert_eq!(store.len(), 3);
    }

    #[test]
    fn test_ewma_convergence() {
        let mut store = EwmaStore::default();
        for _ in 0..20 {
            store.update(1, 50.0, 2.0);
        }
        // After 20 samples, delta should be essentially zero
        let (cpu_d, ram_d) = store.update(1, 50.0, 2.0);
        assert!(cpu_d < 0.01, "converged EWMA should have near-zero delta");
        assert!(ram_d < 0.001, "converged EWMA should have near-zero delta");
    }

    // ── Multi-timescale tests ─────────────────────────────────────────────

    #[test]
    fn test_fast_delta_available_after_2_samples() {
        let mut store = EwmaStore::default();
        store.update(1, 10.0, 1.0);
        let baseline = store.get(1).unwrap();
        assert_eq!(baseline.fast_delta(90.0, 5.0), (0.0, 0.0)); // only 1 sample

        store.update(1, 10.0, 1.0);
        let baseline = store.get(1).unwrap();
        let (cpu_d, ram_d) = baseline.fast_delta(90.0, 5.0);
        assert!(cpu_d > 50.0, "fast delta should detect spike after 2 samples");
        assert!(ram_d > 3.0, "fast RAM delta should detect spike");
    }

    #[test]
    fn test_slow_delta_available_after_8_samples() {
        let mut store = EwmaStore::default();
        for i in 0..8 {
            store.update(1, 10.0, 1.0);
            let baseline = store.get(1).unwrap();
            if i < 7 {
                assert_eq!(
                    baseline.slow_delta(90.0, 5.0),
                    (0.0, 0.0),
                    "slow delta should not report before 8 samples"
                );
            }
        }
        let baseline = store.get(1).unwrap();
        let (cpu_d, _) = baseline.slow_delta(90.0, 5.0);
        assert!(cpu_d > 50.0, "slow delta should detect spike after 8 samples");
    }

    #[test]
    fn test_drift_detection_gradual_increase() {
        let mut store = EwmaStore::default();
        // Gradually ramp CPU from 10 to 80 over 30 samples
        for i in 0..30 {
            let cpu = 10.0 + (i as f64) * 2.5; // 10 → 82.5
            store.update(1, cpu, 1.0);
        }
        let baseline = store.get(1).unwrap();
        // Fast EWMA tracks recent high values closely. Slow EWMA lags behind.
        // The spread should trigger drift detection.
        assert!(
            baseline.drift_detected(),
            "gradual CPU ramp should trigger drift detection (fast={:.1}, slow={:.1})",
            baseline.fast.cpu,
            baseline.slow.cpu
        );
    }

    #[test]
    fn test_no_drift_on_stable_process() {
        let mut store = EwmaStore::default();
        for _ in 0..30 {
            store.update(1, 50.0, 2.0);
        }
        let baseline = store.get(1).unwrap();
        assert!(
            !baseline.drift_detected(),
            "stable process should not trigger drift detection"
        );
    }

    #[test]
    fn test_fast_reacts_faster_than_slow() {
        let mut store = EwmaStore::default();
        // Build stable baseline
        for _ in 0..10 {
            store.update(1, 10.0, 1.0);
        }
        // Sudden spike
        store.update(1, 90.0, 5.0);
        let baseline = store.get(1).unwrap();
        // Fast EWMA should have moved much more than slow
        assert!(
            baseline.fast.cpu > baseline.slow.cpu + 10.0,
            "fast ({:.1}) should react more than slow ({:.1}) to a spike",
            baseline.fast.cpu,
            baseline.slow.cpu
        );
    }
}
