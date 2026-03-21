use std::collections::HashMap;

/// Smoothing factor: α=0.2 gives a ~5-sample window (~10 seconds at 2s ticks).
/// Recent values weigh more; old values fade exponentially.
const ALPHA: f64 = 0.2;

/// Per-process EWMA baseline tracker.
#[derive(Debug, Clone)]
pub struct ProcessBaseline {
    pub cpu_ewma: f64,
    pub ram_ewma: f64,
    initialized: bool,
    pub samples: u32,
}

impl ProcessBaseline {
    fn new() -> Self {
        Self {
            cpu_ewma: 0.0,
            ram_ewma: 0.0,
            initialized: false,
            samples: 0,
        }
    }

    /// Update the baseline with a new observation. Returns (cpu_delta, ram_delta).
    pub fn update(&mut self, cpu: f64, ram: f64) -> (f64, f64) {
        if !self.initialized {
            self.cpu_ewma = cpu;
            self.ram_ewma = ram;
            self.initialized = true;
            self.samples += 1;
            return (0.0, 0.0);
        }
        self.cpu_ewma = ALPHA * cpu + (1.0 - ALPHA) * self.cpu_ewma;
        self.ram_ewma = ALPHA * ram + (1.0 - ALPHA) * self.ram_ewma;
        self.samples += 1;

        // Only report delta after 3+ samples to avoid noise from process startup
        if self.samples < 3 {
            return (0.0, 0.0);
        }
        let cpu_delta = (cpu - self.cpu_ewma).max(0.0);
        let ram_delta = (ram - self.ram_ewma).max(0.0);
        (cpu_delta, ram_delta)
    }
}

/// Stores per-process EWMA baselines, keyed by PID.
#[derive(Debug, Default)]
pub struct EwmaStore {
    baselines: HashMap<u32, ProcessBaseline>,
}

impl EwmaStore {
    /// Update baseline for a process and return (cpu_delta, ram_delta).
    pub fn update(&mut self, pid: u32, cpu: f64, ram: f64) -> (f64, f64) {
        let baseline = self
            .baselines
            .entry(pid)
            .or_insert_with(ProcessBaseline::new);
        baseline.update(cpu, ram)
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
}
