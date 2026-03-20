use crate::types::*;

/// Detect alerts from state transitions.
/// Returns a list of alerts triggered by the transition from previous to current state.
pub fn detect_alerts(
    prev_ram_pressure: &RamPressure,
    ram_pressure: &RamPressure,
    prev_throttling: bool,
    throttling: bool,
    die_temp: Option<f64>,
    ram_used_gb: f64,
    ram_total_gb: f64,
    prev_impact_level: &ImpactLevel,
    impact_level: &ImpactLevel,
    impact_message: &str,
) -> Vec<Alert> {
    let mut alerts = Vec::new();
    let now = chrono::Utc::now();

    // RAM pressure escalation
    if ram_pressure != prev_ram_pressure {
        match (prev_ram_pressure, ram_pressure) {
            (RamPressure::Normal, RamPressure::Warn) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    message: format!(
                        "RAM pressure elevated to warn ({:.1}/{:.0}GB used).",
                        ram_used_gb, ram_total_gb
                    ),
                    ts: now,
                });
            }
            (_, RamPressure::Critical) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    message: format!(
                        "RAM pressure critical ({:.1}/{:.0}GB used). System may freeze.",
                        ram_used_gb, ram_total_gb
                    ),
                    ts: now,
                });
            }
            _ => {}
        }
    }

    // Thermal throttling onset
    if throttling && !prev_throttling {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            message: format!(
                "CPU thermal throttling active ({:.0}°C). Performance is degraded.",
                die_temp.unwrap_or(0.0)
            ),
            ts: now,
        });
    }

    // Impact level escalation
    if impact_level != prev_impact_level {
        match (prev_impact_level, impact_level) {
            (_, ImpactLevel::Strained) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    message: impact_message.to_string(),
                    ts: now,
                });
            }
            (_, ImpactLevel::Critical) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    message: impact_message.to_string(),
                    ts: now,
                });
            }
            _ => {}
        }
    }

    alerts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ram_normal_to_warn() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Warn,
            false,
            false,
            None,
            6.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert!(alerts[0].message.contains("warn"));
    }

    #[test]
    fn test_ram_warn_to_critical() {
        let alerts = detect_alerts(
            &RamPressure::Warn,
            &RamPressure::Critical,
            false,
            false,
            None,
            7.5,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert!(alerts[0].message.contains("critical"));
    }

    #[test]
    fn test_ram_no_change_no_alert() {
        let alerts = detect_alerts(
            &RamPressure::Warn,
            &RamPressure::Warn,
            false,
            false,
            None,
            6.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_ram_critical_to_normal_no_alert() {
        let alerts = detect_alerts(
            &RamPressure::Critical,
            &RamPressure::Normal,
            false,
            false,
            None,
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_throttle_onset() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            true,
            Some(98.0),
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert!(alerts[0].message.contains("throttling"));
        assert!(alerts[0].message.contains("98"));
    }

    #[test]
    fn test_throttle_already_on_no_alert() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Normal,
            true,
            true,
            Some(98.0),
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_impact_escalation_to_strained() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            false,
            None,
            4.0,
            8.0,
            &ImpactLevel::Degrading,
            &ImpactLevel::Strained,
            "System is heavily loaded.",
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].message, "System is heavily loaded.");
    }

    #[test]
    fn test_impact_escalation_to_critical() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            false,
            None,
            4.0,
            8.0,
            &ImpactLevel::Strained,
            &ImpactLevel::Critical,
            "System is at its limit.",
        );
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
    }

    #[test]
    fn test_multiple_alerts_simultaneously() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Critical,
            false,
            true,
            Some(100.0),
            7.5,
            8.0,
            &ImpactLevel::Degrading,
            &ImpactLevel::Critical,
            "Everything is on fire.",
        );
        assert_eq!(alerts.len(), 3); // ram + throttle + impact
    }

    #[test]
    fn test_no_alerts_when_healthy() {
        let alerts = detect_alerts(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            false,
            Some(50.0),
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        assert!(alerts.is_empty());
    }
}
