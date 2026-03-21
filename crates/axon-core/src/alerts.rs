use crate::types::*;

/// Context passed to detect_alerts for populating rich metadata.
pub struct AlertContext<'a> {
    pub prev_ram_pressure: &'a RamPressure,
    pub ram_pressure: &'a RamPressure,
    pub prev_throttling: bool,
    pub throttling: bool,
    pub die_temp: Option<f64>,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub cpu_pct: f64,
    pub prev_disk_pressure: &'a DiskPressure,
    pub disk_pressure: &'a DiskPressure,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
    pub prev_impact_level: &'a ImpactLevel,
    pub impact_level: &'a ImpactLevel,
    pub impact_message: &'a str,
    pub culprit: Option<&'a ProcessInfo>,
    pub culprit_group: Option<&'a ProcessGroup>,
}

fn build_metadata(ctx: &AlertContext) -> AlertMetadata {
    let ram_pct = if ctx.ram_total_gb > 0.0 {
        Some(ctx.ram_used_gb / ctx.ram_total_gb * 100.0)
    } else {
        None
    };
    let disk_pct = if ctx.disk_total_gb > 0.0 {
        Some(ctx.disk_used_gb / ctx.disk_total_gb * 100.0)
    } else {
        None
    };
    AlertMetadata {
        ram_pct,
        cpu_pct: Some(ctx.cpu_pct),
        temp_c: ctx.die_temp,
        disk_pct,
        culprit: ctx.culprit.cloned(),
        culprit_group: ctx.culprit_group.cloned(),
    }
}

/// Detect alerts from state transitions.
/// Returns a list of alerts triggered by the transition from previous to current state.
pub fn detect_alerts(ctx: &AlertContext) -> Vec<Alert> {
    let mut alerts = Vec::new();
    let now = chrono::Utc::now();
    let metadata = build_metadata(ctx);

    // RAM pressure escalation
    if ctx.ram_pressure != ctx.prev_ram_pressure {
        match (ctx.prev_ram_pressure, ctx.ram_pressure) {
            (RamPressure::Normal, RamPressure::Warn) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    alert_type: AlertType::MemoryPressure,
                    message: format!(
                        "RAM pressure elevated to warn ({:.1}/{:.0}GB used).",
                        ctx.ram_used_gb, ctx.ram_total_gb
                    ),
                    ts: now,
                    metadata: metadata.clone(),
                });
            }
            (_, RamPressure::Critical) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    alert_type: AlertType::MemoryPressure,
                    message: format!(
                        "RAM pressure critical ({:.1}/{:.0}GB used). System may freeze.",
                        ctx.ram_used_gb, ctx.ram_total_gb
                    ),
                    ts: now,
                    metadata: metadata.clone(),
                });
            }
            _ => {}
        }
    }

    // Thermal throttling onset
    if ctx.throttling && !ctx.prev_throttling {
        alerts.push(Alert {
            severity: AlertSeverity::Critical,
            alert_type: AlertType::ThermalThrottle,
            message: format!(
                "CPU thermal throttling active ({:.0}°C). Performance is degraded.",
                ctx.die_temp.unwrap_or(0.0)
            ),
            ts: now,
            metadata: metadata.clone(),
        });
    }

    // Disk pressure escalation
    if ctx.disk_pressure != ctx.prev_disk_pressure {
        let disk_pct = if ctx.disk_total_gb > 0.0 {
            ctx.disk_used_gb / ctx.disk_total_gb * 100.0
        } else {
            0.0
        };
        match (ctx.prev_disk_pressure, ctx.disk_pressure) {
            (DiskPressure::Normal, DiskPressure::Warn) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    alert_type: AlertType::DiskPressure,
                    message: format!(
                        "Disk usage elevated to warn ({:.0}/{:.0}GB, {:.0}% used).",
                        ctx.disk_used_gb, ctx.disk_total_gb, disk_pct
                    ),
                    ts: now,
                    metadata: metadata.clone(),
                });
            }
            (_, DiskPressure::Critical) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    alert_type: AlertType::DiskPressure,
                    message: format!(
                        "Disk usage critical ({:.0}/{:.0}GB, {:.0}% used). Free space is running low.",
                        ctx.disk_used_gb, ctx.disk_total_gb, disk_pct
                    ),
                    ts: now,
                    metadata: metadata.clone(),
                });
            }
            _ => {}
        }
    }

    // Impact level escalation
    if ctx.impact_level != ctx.prev_impact_level {
        match (ctx.prev_impact_level, ctx.impact_level) {
            (_, ImpactLevel::Strained) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Warning,
                    alert_type: AlertType::ImpactEscalation,
                    message: ctx.impact_message.to_string(),
                    ts: now,
                    metadata: metadata.clone(),
                });
            }
            (_, ImpactLevel::Critical) => {
                alerts.push(Alert {
                    severity: AlertSeverity::Critical,
                    alert_type: AlertType::ImpactEscalation,
                    message: ctx.impact_message.to_string(),
                    ts: now,
                    metadata,
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

    fn make_ctx<'a>(
        prev_ram: &'a RamPressure,
        ram: &'a RamPressure,
        prev_throttle: bool,
        throttle: bool,
        temp: Option<f64>,
        ram_used: f64,
        ram_total: f64,
        prev_impact: &'a ImpactLevel,
        impact: &'a ImpactLevel,
        msg: &'a str,
    ) -> AlertContext<'a> {
        AlertContext {
            prev_ram_pressure: prev_ram,
            ram_pressure: ram,
            prev_throttling: prev_throttle,
            throttling: throttle,
            die_temp: temp,
            ram_used_gb: ram_used,
            ram_total_gb: ram_total,
            cpu_pct: 50.0,
            prev_disk_pressure: &DiskPressure::Normal,
            disk_pressure: &DiskPressure::Normal,
            disk_used_gb: 250.0,
            disk_total_gb: 500.0,
            prev_impact_level: prev_impact,
            impact_level: impact,
            impact_message: msg,
            culprit: None,
            culprit_group: None,
        }
    }

    fn make_disk_ctx<'a>(
        prev_disk: &'a DiskPressure,
        disk: &'a DiskPressure,
        disk_used: f64,
        disk_total: f64,
    ) -> AlertContext<'a> {
        AlertContext {
            prev_ram_pressure: &RamPressure::Normal,
            ram_pressure: &RamPressure::Normal,
            prev_throttling: false,
            throttling: false,
            die_temp: None,
            ram_used_gb: 4.0,
            ram_total_gb: 8.0,
            cpu_pct: 30.0,
            prev_disk_pressure: prev_disk,
            disk_pressure: disk,
            disk_used_gb: disk_used,
            disk_total_gb: disk_total,
            prev_impact_level: &ImpactLevel::Healthy,
            impact_level: &ImpactLevel::Healthy,
            impact_message: "",
            culprit: None,
            culprit_group: None,
        }
    }

    #[test]
    fn test_ram_normal_to_warn() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].alert_type, AlertType::MemoryPressure);
        assert!(alerts[0].message.contains("warn"));
        assert!(alerts[0].metadata.ram_pct.is_some());
    }

    #[test]
    fn test_ram_warn_to_critical() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].alert_type, AlertType::MemoryPressure);
        assert!(alerts[0].message.contains("critical"));
    }

    #[test]
    fn test_ram_no_change_no_alert() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_ram_critical_to_normal_no_alert() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_throttle_onset() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].alert_type, AlertType::ThermalThrottle);
        assert!(alerts[0].message.contains("throttling"));
        assert!(alerts[0].message.contains("98"));
        assert_eq!(alerts[0].metadata.temp_c, Some(98.0));
    }

    #[test]
    fn test_throttle_already_on_no_alert() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_impact_escalation_to_strained() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
        assert_eq!(alerts[0].message, "System is heavily loaded.");
    }

    #[test]
    fn test_impact_escalation_to_critical() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
    }

    #[test]
    fn test_multiple_alerts_simultaneously() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 3); // ram + throttle + impact
    }

    #[test]
    fn test_no_alerts_when_healthy() {
        let ctx = make_ctx(
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
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_alert_metadata_has_culprit() {
        let culprit = ProcessInfo {
            pid: 1234,
            cmd: "chrome".to_string(),
            cpu_pct: 85.0,
            ram_gb: 2.5,
            blame_score: 0.9,
        };
        let ctx = AlertContext {
            prev_ram_pressure: &RamPressure::Normal,
            ram_pressure: &RamPressure::Critical,
            prev_throttling: false,
            throttling: false,
            die_temp: Some(72.0),
            ram_used_gb: 7.5,
            ram_total_gb: 8.0,
            cpu_pct: 85.0,
            prev_disk_pressure: &DiskPressure::Normal,
            disk_pressure: &DiskPressure::Normal,
            disk_used_gb: 250.0,
            disk_total_gb: 500.0,
            prev_impact_level: &ImpactLevel::Healthy,
            impact_level: &ImpactLevel::Healthy,
            impact_message: "",
            culprit: Some(&culprit),
            culprit_group: None,
        };
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        let meta = &alerts[0].metadata;
        assert!(meta.culprit.is_some());
        assert_eq!(meta.culprit.as_ref().unwrap().pid, 1234);
        assert_eq!(meta.cpu_pct, Some(85.0));
        assert_eq!(meta.temp_c, Some(72.0));
    }

    #[test]
    fn test_alert_type_classification() {
        assert_eq!(format!("{}", AlertType::MemoryPressure), "memory_pressure");
        assert_eq!(
            format!("{}", AlertType::ThermalThrottle),
            "thermal_throttle"
        );
        assert_eq!(
            format!("{}", AlertType::ImpactEscalation),
            "impact_escalation"
        );
        assert_eq!(format!("{}", AlertSeverity::Warning), "warning");
        assert_eq!(format!("{}", AlertSeverity::Critical), "critical");
    }

    #[test]
    fn test_alert_metadata_populated() {
        let culprit = ProcessInfo {
            pid: 42,
            cmd: "app".to_string(),
            cpu_pct: 33.0,
            ram_gb: 1.0,
            blame_score: 0.5,
        };
        let ctx = AlertContext {
            prev_ram_pressure: &RamPressure::Normal,
            ram_pressure: &RamPressure::Warn,
            prev_throttling: false,
            throttling: false,
            die_temp: Some(60.0),
            ram_used_gb: 5.6,
            ram_total_gb: 8.0,
            cpu_pct: 44.0,
            prev_disk_pressure: &DiskPressure::Normal,
            disk_pressure: &DiskPressure::Normal,
            disk_used_gb: 250.0,
            disk_total_gb: 500.0,
            prev_impact_level: &ImpactLevel::Healthy,
            impact_level: &ImpactLevel::Healthy,
            impact_message: "",
            culprit: Some(&culprit),
            culprit_group: None,
        };
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        let m = &alerts[0].metadata;
        assert!(m.ram_pct.unwrap() > 69.0);
        assert_eq!(m.cpu_pct, Some(44.0));
        assert_eq!(m.culprit.as_ref().unwrap().pid, 42);
    }

    #[test]
    fn test_alert_metadata_thermal() {
        let ctx = make_ctx(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            true,
            Some(99.0),
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Healthy,
            "",
        );
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts[0].alert_type, AlertType::ThermalThrottle);
        assert_eq!(alerts[0].metadata.temp_c, Some(99.0));
    }

    #[test]
    fn test_alert_metadata_impact() {
        let ctx = make_ctx(
            &RamPressure::Normal,
            &RamPressure::Normal,
            false,
            false,
            None,
            4.0,
            8.0,
            &ImpactLevel::Healthy,
            &ImpactLevel::Strained,
            "escalated",
        );
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
        assert_eq!(alerts[0].message, "escalated");
    }

    #[test]
    fn test_disk_normal_to_warn() {
        let ctx = make_disk_ctx(&DiskPressure::Normal, &DiskPressure::Warn, 420.0, 500.0);
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Warning);
        assert_eq!(alerts[0].alert_type, AlertType::DiskPressure);
        assert!(alerts[0].message.contains("warn"));
        assert!(alerts[0].metadata.disk_pct.is_some());
    }

    #[test]
    fn test_disk_warn_to_critical() {
        let ctx = make_disk_ctx(&DiskPressure::Warn, &DiskPressure::Critical, 460.0, 500.0);
        let alerts = detect_alerts(&ctx);
        assert_eq!(alerts.len(), 1);
        assert_eq!(alerts[0].severity, AlertSeverity::Critical);
        assert_eq!(alerts[0].alert_type, AlertType::DiskPressure);
        assert!(alerts[0].message.contains("critical"));
    }

    #[test]
    fn test_disk_no_change_no_alert() {
        let ctx = make_disk_ctx(&DiskPressure::Warn, &DiskPressure::Warn, 420.0, 500.0);
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }

    #[test]
    fn test_disk_critical_to_normal_no_alert() {
        let ctx = make_disk_ctx(&DiskPressure::Critical, &DiskPressure::Normal, 200.0, 500.0);
        let alerts = detect_alerts(&ctx);
        assert!(alerts.is_empty());
    }
}
