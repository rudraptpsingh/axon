//! Central tuning for hardware pressure and impact triggers.
//! Lower values here make warnings and impact escalation easier to reach.
//! Thresholds can be overridden at runtime via `ThresholdOverrides::init()`.

use crate::types::{DiskPressure, RamPressure};
use std::sync::OnceLock;

// ── Runtime Overrides ───────────────────────────────────────────────────────

/// Optional per-machine threshold overrides loaded from config file or CLI.
/// Unset fields fall back to the compiled-in constants.
#[derive(Debug, Clone, Default, serde::Deserialize, serde::Serialize)]
pub struct ThresholdOverrides {
    pub ram_warn_pct: Option<f64>,
    pub ram_critical_pct: Option<f64>,
    pub disk_warn_pct: Option<f64>,
    pub disk_critical_pct: Option<f64>,
    pub thermal_throttle_c: Option<f64>,
}

static OVERRIDES: OnceLock<ThresholdOverrides> = OnceLock::new();

/// Initialize runtime threshold overrides (call once at startup).
/// If not called, all thresholds use compiled-in constants.
pub fn init_overrides(overrides: ThresholdOverrides) {
    let _ = OVERRIDES.set(overrides);
}

/// Get the effective RAM warn threshold (override or constant).
pub fn ram_warn() -> f64 {
    OVERRIDES
        .get()
        .and_then(|o| o.ram_warn_pct)
        .unwrap_or(RAM_PCT_WARN)
}

/// Get the effective RAM critical threshold.
pub fn ram_critical() -> f64 {
    OVERRIDES
        .get()
        .and_then(|o| o.ram_critical_pct)
        .unwrap_or(RAM_PCT_CRITICAL)
}

/// Get the effective disk warn threshold.
pub fn disk_warn() -> f64 {
    OVERRIDES
        .get()
        .and_then(|o| o.disk_warn_pct)
        .unwrap_or(DISK_PCT_WARN)
}

/// Get the effective disk critical threshold.
pub fn disk_critical() -> f64 {
    OVERRIDES
        .get()
        .and_then(|o| o.disk_critical_pct)
        .unwrap_or(DISK_PCT_CRITICAL)
}

/// Get the effective thermal throttle threshold.
pub fn thermal_throttle() -> f64 {
    OVERRIDES
        .get()
        .and_then(|o| o.thermal_throttle_c)
        .unwrap_or(THERMAL_THROTTLE_C)
}

// ── RAM pressure tiers (collector → `RamPressure`, memory alerts) ─────────────

/// RAM % of total used: at or above → warn tier (rising edge).
pub const RAM_PCT_WARN: f64 = 55.0;
/// RAM % of total used: at or above → critical tier (rising edge).
pub const RAM_PCT_CRITICAL: f64 = 75.0;

// ── RAM hysteresis (falling edge uses lower thresholds to prevent oscillation)
/// RAM must drop below this to fall from Warn → Normal.
pub const RAM_PCT_WARN_FALLING: f64 = 50.0;
/// RAM must drop below this to fall from Critical → Warn.
pub const RAM_PCT_CRITICAL_FALLING: f64 = 70.0;

// ── Disk pressure tiers (collector → `DiskPressure`, disk alerts) ────────────

/// Disk % of total used: at or above → warn tier (rising edge).
pub const DISK_PCT_WARN: f64 = 80.0;
/// Disk % of total used: at or above → critical tier (rising edge).
pub const DISK_PCT_CRITICAL: f64 = 90.0;

// ── Disk hysteresis (falling edge)
/// Disk must drop below this to fall from Warn → Normal.
pub const DISK_PCT_WARN_FALLING: f64 = 75.0;
/// Disk must drop below this to fall from Critical → Warn.
pub const DISK_PCT_CRITICAL_FALLING: f64 = 85.0;

// ── Thermal ─────────────────────────────────────────────────────────────────

/// Die temperature (°C) above which we flag CPU thermal throttling.
pub const THERMAL_THROTTLE_C: f64 = 85.0;
/// Thermal hysteresis: must drop below this to clear throttling flag.
pub const THERMAL_THROTTLE_FALLING_C: f64 = 80.0;

// ── Alert debounce & flap detection ─────────────────────────────────────────

/// Consecutive ticks at the new level before confirming a state transition.
pub const ALERT_DEBOUNCE_TICKS: u32 = 2;
/// Window (in ticks) over which to count oscillations for flap detection.
pub const FLAP_WINDOW_TICKS: u32 = 15; // 30 seconds at 2s/tick
/// Threshold: >N boundary crossings in the flap window → suppress alerts.
pub const FLAP_THRESHOLD: u32 = 3;

// ── Anomaly type classification (`detect_anomaly_type`, priority order) ───────

pub const ANOMALY_TEMP_C: f64 = 85.0;
pub const ANOMALY_RAM_PCT: f64 = 72.0;
pub const ANOMALY_CPU_PCT: f64 = 72.0;
/// "General slowdown" when neither memory nor CPU alone crosses the strong thresholds.
pub const ANOMALY_GENERAL_RAM_OR_CPU_PCT: f64 = 52.0;

// ── Impact score persistence and level bands ────────────────────────────────

/// Consecutive 2s samples with score above `IMPACT_SCORE_ELEVATED` before reporting non-Healthy impact.
pub const IMPACT_PERSISTENCE_SAMPLES: u32 = 2;

/// Sample counts as "elevated" for persistence if `compute_score` exceeds this.
pub const IMPACT_SCORE_ELEVATED: f64 = 0.20;

/// After persistence is satisfied, map score to level using these upper bounds.
pub const IMPACT_LEVEL_HEALTHY_BELOW: f64 = 0.20;
pub const IMPACT_LEVEL_DEGRADING_BELOW: f64 = 0.38;
pub const IMPACT_LEVEL_STRAINED_BELOW: f64 = 0.55;

// ── Pure classification (used by collector; unit-tested at boundary values) ───

/// Map total-RAM-used percentage to pressure tier (no hysteresis — used by one-shot probes).
/// Uses runtime overrides if set, otherwise compiled-in constants.
pub fn ram_pressure_from_pct(ram_pct: f64) -> RamPressure {
    if ram_pct >= ram_critical() {
        RamPressure::Critical
    } else if ram_pct >= ram_warn() {
        RamPressure::Warn
    } else {
        RamPressure::Normal
    }
}

/// Map RAM % to pressure tier with hysteresis.
/// Rising thresholds differ from falling to prevent oscillation at boundary values.
/// Uses runtime overrides if set.
pub fn ram_pressure_with_hysteresis(ram_pct: f64, prev: &RamPressure) -> RamPressure {
    let warn_rising = ram_warn();
    let warn_falling = warn_rising - 5.0; // 5% hysteresis gap
    let crit_rising = ram_critical();
    let crit_falling = crit_rising - 5.0;
    match prev {
        RamPressure::Normal => {
            if ram_pct >= crit_rising {
                RamPressure::Critical
            } else if ram_pct >= warn_rising {
                RamPressure::Warn
            } else {
                RamPressure::Normal
            }
        }
        RamPressure::Warn => {
            if ram_pct >= crit_rising {
                RamPressure::Critical
            } else if ram_pct < warn_falling {
                RamPressure::Normal
            } else {
                RamPressure::Warn
            }
        }
        RamPressure::Critical => {
            if ram_pct < warn_falling {
                RamPressure::Normal
            } else if ram_pct < crit_falling {
                RamPressure::Warn
            } else {
                RamPressure::Critical
            }
        }
    }
}

/// Map total-disk-used percentage to pressure tier (no hysteresis).
pub fn disk_pressure_from_pct(disk_pct: f64) -> DiskPressure {
    if disk_pct >= disk_critical() {
        DiskPressure::Critical
    } else if disk_pct >= disk_warn() {
        DiskPressure::Warn
    } else {
        DiskPressure::Normal
    }
}

/// Map disk % to pressure tier with hysteresis.
pub fn disk_pressure_with_hysteresis(disk_pct: f64, prev: &DiskPressure) -> DiskPressure {
    let warn_rising = disk_warn();
    let warn_falling = warn_rising - 5.0;
    let crit_rising = disk_critical();
    let crit_falling = crit_rising - 5.0;
    match prev {
        DiskPressure::Normal => {
            if disk_pct >= crit_rising {
                DiskPressure::Critical
            } else if disk_pct >= warn_rising {
                DiskPressure::Warn
            } else {
                DiskPressure::Normal
            }
        }
        DiskPressure::Warn => {
            if disk_pct >= crit_rising {
                DiskPressure::Critical
            } else if disk_pct < warn_falling {
                DiskPressure::Normal
            } else {
                DiskPressure::Warn
            }
        }
        DiskPressure::Critical => {
            if disk_pct < warn_falling {
                DiskPressure::Normal
            } else if disk_pct < crit_falling {
                DiskPressure::Warn
            } else {
                DiskPressure::Critical
            }
        }
    }
}

/// True when die temperature indicates thermal throttling (no hysteresis).
pub fn thermal_throttling_from_temp_c(temp_c: Option<f64>) -> bool {
    temp_c.is_some_and(|t| t > thermal_throttle())
}

/// Thermal throttling with hysteresis.
pub fn thermal_throttling_with_hysteresis(temp_c: Option<f64>, prev_throttling: bool) -> bool {
    let rising = thermal_throttle();
    let falling = rising - 5.0; // 5C hysteresis gap
    match temp_c {
        Some(t) => {
            if prev_throttling {
                t > falling
            } else {
                t > rising
            }
        }
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::impact::detect_anomaly_type;
    use crate::impact::score_to_level;
    use crate::types::{AnomalyType, ImpactLevel};

    #[test]
    fn ram_pressure_boundaries_match_constants() {
        assert_eq!(ram_pressure_from_pct(54.9), RamPressure::Normal);
        assert_eq!(ram_pressure_from_pct(55.0), RamPressure::Warn);
        assert_eq!(ram_pressure_from_pct(74.9), RamPressure::Warn);
        assert_eq!(ram_pressure_from_pct(75.0), RamPressure::Critical);
    }

    #[test]
    fn ram_hysteresis_rising_uses_standard_thresholds() {
        // From Normal, rising thresholds apply
        assert_eq!(
            ram_pressure_with_hysteresis(54.9, &RamPressure::Normal),
            RamPressure::Normal
        );
        assert_eq!(
            ram_pressure_with_hysteresis(55.0, &RamPressure::Normal),
            RamPressure::Warn
        );
        assert_eq!(
            ram_pressure_with_hysteresis(75.0, &RamPressure::Normal),
            RamPressure::Critical
        );
    }

    #[test]
    fn ram_hysteresis_falling_uses_lower_thresholds() {
        // From Warn, must drop to 50% to go back to Normal (not 55%)
        assert_eq!(
            ram_pressure_with_hysteresis(52.0, &RamPressure::Warn),
            RamPressure::Warn
        );
        assert_eq!(
            ram_pressure_with_hysteresis(49.9, &RamPressure::Warn),
            RamPressure::Normal
        );
        // From Critical, must drop to 70% to go to Warn (not 75%)
        assert_eq!(
            ram_pressure_with_hysteresis(72.0, &RamPressure::Critical),
            RamPressure::Critical
        );
        assert_eq!(
            ram_pressure_with_hysteresis(69.9, &RamPressure::Critical),
            RamPressure::Warn
        );
        // From Critical, must drop to 50% to go to Normal
        assert_eq!(
            ram_pressure_with_hysteresis(49.9, &RamPressure::Critical),
            RamPressure::Normal
        );
    }

    #[test]
    fn disk_pressure_boundaries_match_constants() {
        assert_eq!(disk_pressure_from_pct(79.9), DiskPressure::Normal);
        assert_eq!(disk_pressure_from_pct(80.0), DiskPressure::Warn);
        assert_eq!(disk_pressure_from_pct(89.9), DiskPressure::Warn);
        assert_eq!(disk_pressure_from_pct(90.0), DiskPressure::Critical);
    }

    #[test]
    fn disk_hysteresis_falling_uses_lower_thresholds() {
        // From Warn, must drop to 75% to go back to Normal (not 80%)
        assert_eq!(
            disk_pressure_with_hysteresis(77.0, &DiskPressure::Warn),
            DiskPressure::Warn
        );
        assert_eq!(
            disk_pressure_with_hysteresis(74.9, &DiskPressure::Warn),
            DiskPressure::Normal
        );
        // From Critical, must drop to 85% to go to Warn (not 90%)
        assert_eq!(
            disk_pressure_with_hysteresis(87.0, &DiskPressure::Critical),
            DiskPressure::Critical
        );
        assert_eq!(
            disk_pressure_with_hysteresis(84.9, &DiskPressure::Critical),
            DiskPressure::Warn
        );
    }

    #[test]
    fn thermal_throttle_boundary() {
        assert!(!thermal_throttling_from_temp_c(None));
        assert!(!thermal_throttling_from_temp_c(Some(85.0)));
        assert!(thermal_throttling_from_temp_c(Some(85.1)));
    }

    #[test]
    fn thermal_hysteresis() {
        // Rising: needs >85C to trigger
        assert!(!thermal_throttling_with_hysteresis(Some(85.0), false));
        assert!(thermal_throttling_with_hysteresis(Some(85.1), false));
        // Falling: stays on until <=80C
        assert!(thermal_throttling_with_hysteresis(Some(82.0), true));
        assert!(!thermal_throttling_with_hysteresis(Some(80.0), true));
        assert!(!thermal_throttling_with_hysteresis(None, true));
    }

    #[test]
    fn anomaly_detection_uses_same_constants() {
        assert_eq!(
            detect_anomaly_type(72.1, 30.0, None),
            AnomalyType::MemoryPressure
        );
        assert_eq!(
            detect_anomaly_type(71.9, 30.0, None),
            AnomalyType::GeneralSlowdown
        );
        assert_eq!(
            detect_anomaly_type(30.0, 72.1, None),
            AnomalyType::CpuSaturation
        );
        assert_eq!(
            detect_anomaly_type(52.1, 30.0, None),
            AnomalyType::GeneralSlowdown
        );
        assert_eq!(detect_anomaly_type(51.9, 30.0, None), AnomalyType::None);
    }

    #[test]
    fn impact_persistence_and_bands_use_constants() {
        assert_eq!(score_to_level(0.5, 1), ImpactLevel::Healthy);
        assert_eq!(score_to_level(0.5, 2), ImpactLevel::Strained);
        // Below IMPACT_LEVEL_HEALTHY_BELOW (0.20) → Healthy even with persistence
        assert_eq!(score_to_level(0.19, 2), ImpactLevel::Healthy);
        // Above 0.20, below DEGRADING (0.38) → Degrading
        assert_eq!(score_to_level(0.25, 2), ImpactLevel::Degrading);
        // Above 0.38, below STRAINED (0.55) → Strained
        assert_eq!(score_to_level(0.45, 2), ImpactLevel::Strained);
    }
}
