//! One-shot hardware probes using the same sysinfo refresh pattern as the collector.
//! Intended for live tests and scripts that need numbers comparable to `axon serve` snapshots.

use sysinfo::System;

/// Global RAM used as a percentage of total (matches collector `ram_pct`).
pub fn ram_used_pct() -> f64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    let total = sys.total_memory();
    if total == 0 {
        return 0.0;
    }
    sys.used_memory() as f64 / total as f64 * 100.0
}

/// Total RAM in bytes (for computing how much headroom is left to a target %).
pub fn total_memory_bytes() -> u64 {
    let mut sys = System::new_all();
    sys.refresh_memory();
    sys.total_memory()
}

/// Global CPU usage 0–100 (same call sequence as collector tick).
pub fn global_cpu_usage_pct() -> f64 {
    let mut sys = System::new_all();
    sys.refresh_all();
    sys.global_cpu_usage() as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_used_pct_is_in_range() {
        let p = ram_used_pct();
        assert!((0.0..=100.0).contains(&p), "ram_used_pct={}", p);
    }
}
