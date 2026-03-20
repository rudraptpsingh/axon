use crate::types::{AnomalyType, ImpactLevel, ProcessInfo};

// ── Anomaly Detection ─────────────────────────────────────────────────────────

/// Classify the primary anomaly type based on system state.
pub fn detect_anomaly_type(ram_pct: f64, cpu_pct: f64, temp: Option<f64>) -> AnomalyType {
    if temp.map_or(false, |t| t > 95.0) {
        return AnomalyType::ThermalThrottle;
    }
    if ram_pct > 85.0 {
        return AnomalyType::MemoryPressure;
    }
    if cpu_pct > 85.0 {
        return AnomalyType::CpuSaturation;
    }
    if ram_pct > 65.0 || cpu_pct > 65.0 {
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
/// `above_threshold_count` = consecutive samples where score > 0.3.
/// Requires 3+ consecutive samples to avoid false positives on spikes.
pub fn score_to_level(score: f64, above_threshold_count: u32) -> ImpactLevel {
    if above_threshold_count < 3 {
        return ImpactLevel::Healthy;
    }
    if score < 0.30 {
        ImpactLevel::Healthy
    } else if score < 0.50 {
        ImpactLevel::Degrading
    } else if score < 0.75 {
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
pub fn suggest_fix(culprit: Option<&ProcessInfo>, anomaly: &AnomalyType) -> String {
    if let Some(p) = culprit {
        let name = p.cmd.to_lowercase();
        // Strip path prefix and arguments
        let base = name
            .split('/')
            .last()
            .unwrap_or(&name)
            .split_whitespace()
            .next()
            .unwrap_or(&name)
            .trim_end_matches('\0');

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
                Some("Close unused browser tabs to free memory.".to_string())
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
