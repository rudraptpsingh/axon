//! Central tuning for hardware pressure and impact triggers.
//! Lower values here make warnings and impact escalation easier to reach.

use crate::types::RamPressure;

// ── RAM pressure tiers (collector → `RamPressure`, memory alerts) ─────────────

/// RAM % of total used: at or above → warn tier.
pub const RAM_PCT_WARN: f64 = 55.0;
/// RAM % of total used: at or above → critical tier.
pub const RAM_PCT_CRITICAL: f64 = 75.0;

// ── Thermal ─────────────────────────────────────────────────────────────────

/// Die temperature (°C) above which we flag CPU thermal throttling.
pub const THERMAL_THROTTLE_C: f64 = 85.0;

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

/// Map total-RAM-used percentage to pressure tier (same rules as `axon serve` collector loop).
pub fn ram_pressure_from_pct(ram_pct: f64) -> RamPressure {
    if ram_pct >= RAM_PCT_CRITICAL {
        RamPressure::Critical
    } else if ram_pct >= RAM_PCT_WARN {
        RamPressure::Warn
    } else {
        RamPressure::Normal
    }
}

/// True when die temperature indicates thermal throttling (collector `throttling` flag).
pub fn thermal_throttling_from_temp_c(temp_c: Option<f64>) -> bool {
    temp_c.is_some_and(|t| t > THERMAL_THROTTLE_C)
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
    fn thermal_throttle_boundary() {
        assert!(!thermal_throttling_from_temp_c(None));
        assert!(!thermal_throttling_from_temp_c(Some(85.0)));
        assert!(thermal_throttling_from_temp_c(Some(85.1)));
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
