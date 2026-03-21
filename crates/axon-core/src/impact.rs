use crate::thresholds;
use crate::types::{AnomalyType, ImpactLevel, ProcessGroup, ProcessInfo};

// ── Anomaly Detection ─────────────────────────────────────────────────────────

/// Classify the primary anomaly type based on system state.
pub fn detect_anomaly_type(ram_pct: f64, cpu_pct: f64, temp: Option<f64>) -> AnomalyType {
    if temp.is_some_and(|t| t > thresholds::ANOMALY_TEMP_C) {
        return AnomalyType::ThermalThrottle;
    }
    if ram_pct > thresholds::ANOMALY_RAM_PCT {
        return AnomalyType::MemoryPressure;
    }
    if cpu_pct > thresholds::ANOMALY_CPU_PCT {
        return AnomalyType::CpuSaturation;
    }
    if ram_pct > thresholds::ANOMALY_GENERAL_RAM_OR_CPU_PCT
        || cpu_pct > thresholds::ANOMALY_GENERAL_RAM_OR_CPU_PCT
    {
        return AnomalyType::GeneralSlowdown;
    }
    AnomalyType::None
}

/// Compute weighted anomaly score [0.0, 1.0].
/// Uses multi-signal fusion: RAM pressure + CPU saturation + swap usage.
pub fn compute_score(ram_pct: f64, cpu_pct: f64, swap_gb: f64) -> f64 {
    let ram_norm = (ram_pct / 100.0).min(1.0);
    let cpu_norm = (cpu_pct / 100.0).min(1.0);
    let swap_norm = (swap_gb / 8.0).min(1.0); // 8GB swap = saturated
    (0.4 * ram_norm + 0.3 * cpu_norm + 0.3 * swap_norm).min(1.0)
}

/// Map anomaly score → ImpactLevel with persistence check.
/// `above_threshold_count` = consecutive samples where score > `thresholds::IMPACT_SCORE_ELEVATED`.
/// Requires `IMPACT_PERSISTENCE_SAMPLES` consecutive samples to avoid false positives on spikes.
pub fn score_to_level(score: f64, above_threshold_count: u32) -> ImpactLevel {
    if above_threshold_count < thresholds::IMPACT_PERSISTENCE_SAMPLES {
        return ImpactLevel::Healthy;
    }
    if score < thresholds::IMPACT_LEVEL_HEALTHY_BELOW {
        ImpactLevel::Healthy
    } else if score < thresholds::IMPACT_LEVEL_DEGRADING_BELOW {
        ImpactLevel::Degrading
    } else if score < thresholds::IMPACT_LEVEL_STRAINED_BELOW {
        ImpactLevel::Strained
    } else {
        ImpactLevel::Critical
    }
}

// ── Human-Readable Impact Messages ───────────────────────────────────────────

pub fn impact_message(level: &ImpactLevel, anomaly: &AnomalyType) -> String {
    match (level, anomaly) {
        (ImpactLevel::Healthy, _) => "System is healthy. No action needed.".to_string(),

        (ImpactLevel::Degrading, AnomalyType::MemoryPressure) => {
            "Memory is under load. Minor slowdowns possible.".to_string()
        }
        (ImpactLevel::Degrading, AnomalyType::CpuSaturation) => {
            "CPU is under load. Tasks may take slightly longer.".to_string()
        }
        (ImpactLevel::Degrading, _) => {
            "System is under load. You may notice minor slowdowns.".to_string()
        }

        (ImpactLevel::Strained, AnomalyType::MemoryPressure) => {
            "Memory pressure is high. Apps may lag or become unresponsive.".to_string()
        }
        (ImpactLevel::Strained, AnomalyType::ThermalThrottle) => {
            "CPU is thermal throttling. Build and compile performance is reduced.".to_string()
        }
        (ImpactLevel::Strained, AnomalyType::CpuSaturation) => {
            "CPU is heavily loaded. Your system feels sluggish.".to_string()
        }
        (ImpactLevel::Strained, _) => {
            "System is heavily loaded. Expect lags and unresponsiveness.".to_string()
        }

        (ImpactLevel::Critical, AnomalyType::MemoryPressure) => {
            "System is overloaded and swapping heavily. Your session may freeze or crash."
                .to_string()
        }
        (ImpactLevel::Critical, AnomalyType::ThermalThrottle) => {
            "CPU is severely throttling at extreme temperatures. Performance is critically degraded."
                .to_string()
        }
        (ImpactLevel::Critical, _) => {
            "System is at its limit. Expect freezes or a crash soon.".to_string()
        }
    }
}

// ── Fix Suggestions ───────────────────────────────────────────────────────────

/// Return a concrete fix for the given culprit process and anomaly type.
/// When a group is available, uses the group name for matching and includes
/// group-level stats in the fix message.
pub fn suggest_fix(
    culprit: Option<&ProcessInfo>,
    group: Option<&ProcessGroup>,
    anomaly: &AnomalyType,
) -> String {
    // Use group name for matching if available, fall back to culprit cmd
    let match_name = group
        .map(|g| g.name.clone())
        .or_else(|| culprit.map(|p| p.cmd.clone()));

    if let Some(raw_name) = match_name {
        // Strip path prefix and null terminators, keep full name for matching
        let name = raw_name.to_lowercase();
        let base = name
            .split('/')
            .next_back()
            .unwrap_or(&name)
            .trim_end_matches('\0')
            .trim();

        let fix = match base {
            n if n.contains("cursor") => {
                Some("Restart Cursor or close unused editor tabs (Cmd+W).".to_string())
            }
            n if n.contains("cargo") => match anomaly {
                AnomalyType::ThermalThrottle | AnomalyType::CpuSaturation => {
                    Some("Reduce build parallelism: cargo build -j 2".to_string())
                }
                _ => Some("Consider running: cargo build -j 2 to reduce system load.".to_string()),
            },
            "node" | "node.js" => {
                Some("Restart your dev server (Ctrl+C, then npm run dev).".to_string())
            }
            n if n.contains("python") || n == "python3" => {
                Some("Stop the script (Ctrl+C) or reduce batch size.".to_string())
            }
            n if n.contains("docker") => {
                Some("Stop unused containers: docker compose down".to_string())
            }
            "code" | "code helper" | "electron" => {
                Some("Reload VS Code: Cmd+Shift+P → Reload Window".to_string())
            }
            n if n.contains("ollama") || n.contains("llama") || n.contains("mlx") => {
                Some("Pause local inference before running heavy tasks: ollama stop".to_string())
            }
            n if n.contains("windsurf") => {
                Some("Restart Windsurf or close unused editor tabs.".to_string())
            }
            n if n.contains("chrome") || n.contains("safari") || n.contains("firefox") => {
                if let Some(g) = group {
                    if g.process_count > 1 {
                        Some(format!(
                            "Close unused browser tabs to free {:.1}GB ({} processes).",
                            g.total_ram_gb, g.process_count
                        ))
                    } else {
                        Some("Close unused browser tabs to free memory.".to_string())
                    }
                } else {
                    Some("Close unused browser tabs to free memory.".to_string())
                }
            }
            _ => None,
        };

        if let Some(f) = fix {
            return f;
        }
    }

    // Fallback by anomaly type
    match anomaly {
        AnomalyType::MemoryPressure => "Close or restart the heaviest application.".to_string(),
        AnomalyType::CpuSaturation => "Stop or pause the heavy process.".to_string(),
        AnomalyType::ThermalThrottle => {
            "Allow the system to cool for 30 seconds before continuing.".to_string()
        }
        AnomalyType::GeneralSlowdown => {
            "Reduce system load by closing unused applications.".to_string()
        }
        AnomalyType::None => "No action needed.".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_anomaly_thermal() {
        assert_eq!(
            detect_anomaly_type(50.0, 50.0, Some(100.0)),
            AnomalyType::ThermalThrottle
        );
    }

    #[test]
    fn test_detect_anomaly_memory() {
        assert_eq!(
            detect_anomaly_type(90.0, 50.0, None),
            AnomalyType::MemoryPressure
        );
    }

    #[test]
    fn test_detect_anomaly_cpu() {
        assert_eq!(
            detect_anomaly_type(50.0, 90.0, None),
            AnomalyType::CpuSaturation
        );
    }

    #[test]
    fn test_detect_anomaly_general() {
        assert_eq!(
            detect_anomaly_type(70.0, 70.0, None),
            AnomalyType::GeneralSlowdown
        );
    }

    #[test]
    fn test_detect_anomaly_none() {
        assert_eq!(detect_anomaly_type(30.0, 30.0, None), AnomalyType::None);
    }

    #[test]
    fn test_compute_score_bounds() {
        let low = compute_score(0.0, 0.0, 0.0);
        assert!((low - 0.0).abs() < 0.001);

        let high = compute_score(100.0, 100.0, 100.0);
        assert!((high - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_score_to_level_persistence() {
        // Below persistence threshold: always Healthy
        assert_eq!(score_to_level(0.6, 1), ImpactLevel::Healthy);
        // At persistence threshold: maps to level by bands
        // 0.6 >= STRAINED (0.55): Critical? No, < STRAINED_BELOW (0.55) is false, so Strained
        assert_eq!(score_to_level(0.5, 2), ImpactLevel::Strained);
        // 0.4 >= DEGRADING (0.38) and < STRAINED (0.55) → Strained
        assert_eq!(score_to_level(0.4, 5), ImpactLevel::Strained);
        assert_eq!(score_to_level(0.8, 2), ImpactLevel::Critical);
        // 0.19 < HEALTHY_BELOW (0.20) → Healthy
        assert_eq!(score_to_level(0.19, 5), ImpactLevel::Healthy);
        // 0.25 in Degrading band (>= 0.20, < 0.38) → Degrading
        assert_eq!(score_to_level(0.25, 5), ImpactLevel::Degrading);
    }

    #[test]
    fn test_suggest_fix_known_processes() {
        let chrome = ProcessInfo {
            pid: 1,
            cmd: "Google Chrome Helper".to_string(),
            cpu_pct: 50.0,
            ram_gb: 2.0,
            blame_score: 0.5,
        };
        let fix = suggest_fix(Some(&chrome), None, &AnomalyType::MemoryPressure);
        assert!(
            fix.contains("browser") || fix.contains("tabs"),
            "expected browser fix, got: {}",
            fix
        );
    }

    #[test]
    fn test_suggest_fix_with_group() {
        let chrome = ProcessInfo {
            pid: 1,
            cmd: "Google Chrome Helper".to_string(),
            cpu_pct: 50.0,
            ram_gb: 2.0,
            blame_score: 0.5,
        };
        let group = ProcessGroup {
            name: "Google Chrome".to_string(),
            process_count: 47,
            total_cpu_pct: 120.0,
            total_ram_gb: 6.2,
            blame_score: 0.5,
            top_pid: 1,
            pids: vec![1],
        };
        let fix = suggest_fix(Some(&chrome), Some(&group), &AnomalyType::MemoryPressure);
        assert!(
            fix.contains("6.2GB"),
            "expected group RAM in fix, got: {}",
            fix
        );
        assert!(
            fix.contains("47"),
            "expected process count in fix, got: {}",
            fix
        );
    }

    #[test]
    fn test_suggest_fix_fallback() {
        let unknown = ProcessInfo {
            pid: 99,
            cmd: "unknown_app_xyz".to_string(),
            cpu_pct: 50.0,
            ram_gb: 2.0,
            blame_score: 0.5,
        };
        let fix = suggest_fix(Some(&unknown), None, &AnomalyType::MemoryPressure);
        assert_eq!(fix, "Close or restart the heaviest application.");
    }
}
