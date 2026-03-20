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
        let baseline = self.baselines.entry(pid).or_insert_with(ProcessBaseline::new);
        baseline.update(cpu, ram)
    }

    /// Remove baselines for PIDs that are no longer active (avoids unbounded growth).
    pub fn cleanup(&mut self, active_pids: &[u32]) {
        self.baselines.retain(|pid, _| active_pids.contains(pid));
    }
}
