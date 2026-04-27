use crate::thresholds;
use crate::types::{
    AnomalyType, CulpritCategory, DiskPressure, HeadroomLevel, HwSnapshot, ImpactLevel,
    ProcessGroup, ProcessInfo, RamPressure, TrendDirection, Urgency,
};

// ── Headroom Computation ──────────────────────────────────────────────────────

/// Compute headroom level for pre-task checks. Agents call hw_snapshot before
/// starting heavy tasks; headroom tells them whether it is safe to proceed.
pub fn compute_headroom(snap: &HwSnapshot) -> (HeadroomLevel, String) {
    // Priority order: most severe conditions first
    if snap.ram_pressure == RamPressure::Critical {
        let ram_pct = if snap.ram_total_gb > 0.0 {
            snap.ram_used_gb / snap.ram_total_gb * 100.0
        } else {
            0.0
        };
        return (
            HeadroomLevel::Insufficient,
            format!("RAM at {:.0}% (Critical)", ram_pct),
        );
    }
    if snap.disk_pressure == DiskPressure::Critical {
        let disk_pct = if snap.disk_total_gb > 0.0 {
            snap.disk_used_gb / snap.disk_total_gb * 100.0
        } else {
            0.0
        };
        return (
            HeadroomLevel::Insufficient,
            format!("Disk at {:.0}% (Critical)", disk_pct),
        );
    }
    if snap.throttling {
        let temp_str = snap
            .die_temp_celsius
            .map(|t| format!(" at {:.0}C", t))
            .unwrap_or_default();
        return (
            HeadroomLevel::Insufficient,
            format!("CPU thermal throttling{}", temp_str),
        );
    }
    if snap.ram_pressure == RamPressure::Warn && snap.cpu_usage_pct >= 70.0 {
        return (
            HeadroomLevel::Insufficient,
            format!("RAM warn + CPU at {:.0}%", snap.cpu_usage_pct),
        );
    }
    if snap.disk_pressure == DiskPressure::Warn && snap.cpu_usage_pct >= 70.0 {
        return (
            HeadroomLevel::Insufficient,
            format!("Disk warn + CPU at {:.0}%", snap.cpu_usage_pct),
        );
    }
    if snap.ram_pressure == RamPressure::Warn {
        let ram_pct = if snap.ram_total_gb > 0.0 {
            snap.ram_used_gb / snap.ram_total_gb * 100.0
        } else {
            0.0
        };
        return (
            HeadroomLevel::Limited,
            format!("RAM at {:.0}% (Warn)", ram_pct),
        );
    }
    if snap.disk_pressure == DiskPressure::Warn {
        let disk_pct = if snap.disk_total_gb > 0.0 {
            snap.disk_used_gb / snap.disk_total_gb * 100.0
        } else {
            0.0
        };
        return (
            HeadroomLevel::Limited,
            format!("Disk at {:.0}% (Warn)", disk_pct),
        );
    }
    if snap.cpu_usage_pct >= 70.0 {
        return (
            HeadroomLevel::Limited,
            format!("CPU at {:.0}%", snap.cpu_usage_pct),
        );
    }
    (HeadroomLevel::Adequate, "System has headroom".to_string())
}

// ── Agent Accumulation Detection ─────────────────────────────────────────────

/// Known AI agent process names (after normalize_process_name).
const KNOWN_AGENT_NAMES: &[&str] = &["claude", "claude code", "cursor", "windsurf", "code", "zed"];

/// Check if a normalized group name matches a known AI agent.
pub fn is_known_agent(name: &str) -> bool {
    let lower = name.to_lowercase();
    KNOWN_AGENT_NAMES
        .iter()
        .any(|&a| lower == a || lower.contains(a))
}

/// Find the first agent group with more than one instance.
pub fn detect_agent_accumulation(groups: &[ProcessGroup]) -> Option<&ProcessGroup> {
    groups
        .iter()
        .find(|g| g.process_count > 1 && is_known_agent(&g.name))
}

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
/// Uses multi-signal fusion with anomaly-aware weights so that a single
/// saturated signal (e.g. CPU at 100% with low RAM) can still reach the
/// "strained" band instead of being capped at "degrading".
pub fn compute_score(ram_pct: f64, cpu_pct: f64, swap_gb: f64) -> f64 {
    compute_score_with_io(ram_pct, cpu_pct, swap_gb, 0.0)
}

/// Extended score computation that includes I/O wait percentage.
/// `io_wait_pct` is 0-100%; 0.0 disables the I/O signal (backward compatible).
pub fn compute_score_with_io(ram_pct: f64, cpu_pct: f64, swap_gb: f64, io_wait_pct: f64) -> f64 {
    let ram_norm = (ram_pct / 100.0).min(1.0);
    let cpu_norm = (cpu_pct / 100.0).min(1.0);
    let swap_norm = (swap_gb / 8.0).min(1.0); // 8GB swap = saturated
    let io_norm = (io_wait_pct / 100.0).min(1.0);

    // Boost the dominant signal so single-resource saturation is not capped
    // at a low band.
    let has_io = io_wait_pct > 0.0;
    let (w_ram, w_cpu, w_swap, w_io) = if cpu_norm > 0.9 && ram_norm < 0.5 {
        // CPU-dominant: boost CPU weight
        if has_io {
            (0.12, 0.58, 0.15, 0.15)
        } else {
            (0.15, 0.65, 0.20, 0.0)
        }
    } else if ram_norm > 0.7 && cpu_norm < 0.5 {
        // RAM-dominant: boost RAM weight so critical RAM (88%+) reaches Critical band
        if has_io {
            (0.58, 0.12, 0.15, 0.15)
        } else {
            (0.65, 0.15, 0.20, 0.0)
        }
    } else if has_io && io_norm > 0.3 && cpu_norm < 0.5 && ram_norm < 0.5 {
        // I/O-dominant: disk-bound workload (e.g. cargo build on slow disk)
        (0.15, 0.15, 0.15, 0.55)
    } else if has_io {
        // I/O present but not dominant: balanced with I/O weight
        (0.35, 0.25, 0.20, 0.20)
    } else {
        // No I/O data: original balanced weights
        (0.40, 0.30, 0.30, 0.0)
    };

    (w_ram * ram_norm + w_cpu * cpu_norm + w_swap * swap_norm + w_io * io_norm).min(1.0)
}

// ── I/O Wait Reader ─────────────────────────────────────────────────────────

/// Read I/O wait / disk busy percentage from the system.
/// Linux: parses /proc/stat for iowait.
/// Windows: reads PhysicalDisk % Disk Time via `typeperf`.
/// macOS: returns 0.0 (no iowait concept).
pub fn read_io_wait_pct() -> f64 {
    #[cfg(target_os = "linux")]
    {
        read_io_wait_linux()
    }
    #[cfg(target_os = "windows")]
    {
        read_disk_busy_windows()
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows")))]
    {
        0.0
    }
}

#[cfg(target_os = "linux")]
static PREV_IO_STAT: std::sync::Mutex<Option<(u64, u64)>> = std::sync::Mutex::new(None);

#[cfg(target_os = "linux")]
fn read_io_wait_linux() -> f64 {
    // Parse first "cpu " line from /proc/stat:
    // cpu  user nice system idle iowait irq softirq steal guest guest_nice
    let content = match std::fs::read_to_string("/proc/stat") {
        Ok(c) => c,
        Err(_) => return 0.0,
    };
    let line = match content.lines().find(|l| l.starts_with("cpu ")) {
        Some(l) => l,
        None => return 0.0,
    };
    let fields: Vec<u64> = line
        .split_whitespace()
        .skip(1) // skip "cpu"
        .filter_map(|s| s.parse().ok())
        .collect();
    if fields.len() < 5 {
        return 0.0;
    }
    let total: u64 = fields.iter().sum();
    let iowait = fields[4]; // 5th field (0-indexed: 4)

    // Delta-based: compare against previous reading to get instantaneous rate
    let mut prev = PREV_IO_STAT.lock().unwrap();
    let result = match *prev {
        Some((prev_iowait, prev_total)) => {
            let d_total = total.saturating_sub(prev_total);
            let d_iowait = iowait.saturating_sub(prev_iowait);
            if d_total == 0 {
                0.0
            } else {
                (d_iowait as f64 / d_total as f64) * 100.0
            }
        }
        None => 0.0, // first call, no delta available
    };
    *prev = Some((iowait, total));
    result
}

/// Windows: Read disk busy % via PowerShell Get-CimInstance.
/// Returns the PhysicalDisk(_Total) % Disk Time, which is the Windows
/// equivalent of Linux iowait — it measures how busy the disk subsystem is.
///
/// PowerShell startup costs ~0.8s, so we cache the result and only refresh
/// every 5 calls (~10s at 2s tick interval) to avoid blocking the collector.
#[cfg(target_os = "windows")]
static DISK_BUSY_CACHE: std::sync::Mutex<(f64, u32)> = std::sync::Mutex::new((0.0, 0));

#[cfg(target_os = "windows")]
fn read_disk_busy_windows() -> f64 {
    let mut cache = DISK_BUSY_CACHE.lock().unwrap();
    cache.1 += 1;
    // Refresh every 5 ticks (~10s); first call (tick 1) always fetches.
    if cache.1 > 1 && cache.1 % 5 != 0 {
        return cache.0;
    }
    let val = read_disk_busy_windows_inner();
    cache.0 = val;
    val
}

#[cfg(target_os = "windows")]
fn read_disk_busy_windows_inner() -> f64 {
    use std::process::Command;
    let output = match Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_PerfFormattedData_PerfDisk_PhysicalDisk -Filter \"Name='_Total'\").PercentDiskTime",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return 0.0,
    };
    if !output.status.success() {
        return 0.0;
    }
    String::from_utf8_lossy(&output.stdout)
        .trim()
        .parse::<f64>()
        .unwrap_or(0.0)
        .clamp(0.0, 100.0)
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

/// Score → ImpactLevel with CUSUM-based persistence and extreme-signal fast paths.
///
/// `cusum_triggered`: true when the CUSUM accumulator has crossed its threshold,
/// indicating a sustained shift in the anomaly score above the idle baseline.
/// This replaces the old `above_threshold_count` counter.
///
/// Two additional fast paths that bypass CUSUM:
/// 1. `top_process_cpu_pct >= 90.0`: single process pegging a full core.
///    On a 4-core machine, global cpu_pct is only 25% — below every threshold —
///    so without this the scoreboard stays Healthy for a clearly runaway process.
/// 2. `cpu_pct > 90.0 || ram_pct > 85.0`: extreme system-wide signals trigger
///    immediately without waiting for CUSUM accumulation.
pub fn score_to_level_with_context(
    score: f64,
    cusum_triggered: bool,
    cpu_pct: f64,
    ram_pct: f64,
    top_process_cpu_pct: f64,
) -> ImpactLevel {
    // Per-process core saturation fast path: bypass CUSUM entirely.
    // Still uses the system score to distinguish severity bands.
    if top_process_cpu_pct >= 90.0 {
        return if score >= thresholds::IMPACT_LEVEL_STRAINED_BELOW {
            ImpactLevel::Critical
        } else if score >= thresholds::IMPACT_LEVEL_DEGRADING_BELOW {
            ImpactLevel::Strained
        } else {
            ImpactLevel::Degrading
        };
    }

    // Extreme system-wide signals or CUSUM confirmation required to escalate.
    let triggered = cusum_triggered || cpu_pct > 90.0 || ram_pct > 85.0;
    if !triggered {
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
        (ImpactLevel::Healthy, AnomalyType::AgentAccumulation) => {
            "Multiple AI agent instances detected. Combined resource usage is growing.".to_string()
        }
        (ImpactLevel::Healthy, AnomalyType::CpuSaturation) => {
            "CPU usage is elevated. Monitor if it persists.".to_string()
        }
        (ImpactLevel::Healthy, AnomalyType::MemoryPressure) => {
            "Memory usage is elevated. Monitor if it persists.".to_string()
        }
        (ImpactLevel::Healthy, AnomalyType::GeneralSlowdown) => {
            "System load is elevated. Monitor if it persists.".to_string()
        }
        (ImpactLevel::Healthy, _) => "System is healthy. No action needed.".to_string(),

        (_, AnomalyType::AgentAccumulation) => {
            "Multiple AI agent instances detected. Combined resource usage is significant."
                .to_string()
        }

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
            n if n.contains("cargo") || n.contains("rustc") => match anomaly {
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
            n if n.contains("axon") => {
                if let Some(g) = group {
                    if g.process_count > 1 {
                        let self_pid = std::process::id();
                        let stale_pids: Vec<String> = g
                            .pids
                            .iter()
                            .filter(|&&pid| pid != self_pid)
                            .map(|p| p.to_string())
                            .collect();
                        if !stale_pids.is_empty() {
                            Some(format!(
                                "{} stale axon instance(s) from old sessions. Kill them: kill {}",
                                stale_pids.len(),
                                stale_pids.join(" ")
                            ))
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                } else {
                    None
                }
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

    // Agent accumulation: provide specific fix with count and name
    if *anomaly == AnomalyType::AgentAccumulation {
        if let Some(g) = group {
            return format!(
                "{} {} instances are running. Close unused sessions to free ~{:.1}GB and reduce background CPU.",
                g.process_count, g.name, g.total_ram_gb
            );
        }
    }

    // Fallback by anomaly type — always include the culprit name when available
    let name_hint = group
        .map(|g| {
            if g.process_count > 1 {
                format!(
                    "{} ({} processes, {:.1}GB, {:.0}% CPU)",
                    g.name, g.process_count, g.total_ram_gb, g.total_cpu_pct
                )
            } else {
                g.name.clone()
            }
        })
        .or_else(|| culprit.map(|p| format!("{} (PID {})", p.cmd, p.pid)));

    match anomaly {
        AnomalyType::MemoryPressure => {
            if let Some(n) = &name_hint {
                format!("Close or restart {} to free memory.", n)
            } else {
                "Close or restart the heaviest application.".to_string()
            }
        }
        AnomalyType::CpuSaturation => {
            if let Some(n) = &name_hint {
                format!("Stop or pause {} to reduce CPU load.", n)
            } else {
                "Stop or pause the heavy process.".to_string()
            }
        }
        AnomalyType::ThermalThrottle => {
            if let Some(n) = &name_hint {
                format!("Pause {} and allow the system to cool.", n)
            } else {
                "Allow the system to cool for 30 seconds before continuing.".to_string()
            }
        }
        AnomalyType::GeneralSlowdown => {
            if let Some(n) = &name_hint {
                format!("Reduce load from {} or close unused applications.", n)
            } else {
                "Reduce system load by closing unused applications.".to_string()
            }
        }
        AnomalyType::AgentAccumulation => {
            if let Some(n) = &name_hint {
                format!("Close unused sessions of {} to free memory.", n)
            } else {
                "Close unused AI agent sessions to free memory.".to_string()
            }
        }
        AnomalyType::None => "No action needed.".to_string(),
    }
}

// ── Culprit Category Classification ─────────────────────────────────────────

/// Classify a process name into a category for agent context.
/// Order matters: check more specific categories first to avoid false matches.
pub fn classify_culprit(name: &str) -> CulpritCategory {
    let lower = name.to_lowercase();
    let base = lower
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&lower)
        .trim_end_matches(".exe");

    // System processes (check early — these are never build tools or browsers)
    if [
        "kernel",
        "systemd",
        "launchd",
        "svchost",
        "csrss",
        "dwm",
        "wininit",
        "loginwindow",
        "windowserver",
        "kworker",
        "ksoftirqd",
        "init",
        "system idle",
        "system",
        "memory compression",
        "registry",
        "smss",
        "lsass",
        "services",
        "wudfhost",
        "taskhostw",
        "runtimebroker",
        "searchhost",
        "explorer",
        "finder",
        "mds",
        "spotlight",
    ]
    .iter()
    .any(|&s| base == s || lower.contains(s))
    {
        return CulpritCategory::System;
    }

    // Browsers (check before build tools — "chrome" must not match "go")
    if [
        "chrome", "firefox", "safari", "edge", "brave", "opera", "vivaldi", "arc",
    ]
    .iter()
    .any(|&b| base.contains(b))
    {
        return CulpritCategory::Browser;
    }

    // AI agents
    if [
        "claude",
        "copilot",
        "ollama",
        "llama",
        "mlx",
        "llamafile",
        "gpt",
    ]
    .iter()
    .any(|&a| base.contains(a))
    {
        return CulpritCategory::AiAgent;
    }

    // IDEs (check before build tools — "code" is an IDE, not a build tool)
    if [
        "cursor",
        "windsurf",
        "zed",
        "idea",
        "webstorm",
        "pycharm",
        "goland",
        "rider",
        "clion",
        "datagrip",
        "rubymine",
        "phpstorm",
        "android studio",
        "xcode",
        "sublime",
        "atom",
        "emacs",
        "vim",
        "nvim",
        "neovim",
    ]
    .iter()
    .any(|&i| base.contains(i))
    {
        return CulpritCategory::Ide;
    }
    // "code" exact match (VS Code) — separate to avoid matching "barcode" etc
    if base == "code" || base == "code helper" || base.starts_with("code ") {
        return CulpritCategory::Ide;
    }

    // Build tools (use exact base match for short names to avoid false positives)
    let exact_build = [
        "cargo", "rustc", "gcc", "g++", "clang", "make", "cmake", "ninja", "msbuild", "javac",
        "gradle", "maven", "tsc", "go", "dotnet", "swift",
    ];
    if exact_build.contains(&base) {
        return CulpritCategory::BuildTool;
    }
    // Contains-match for longer, unambiguous names
    if ["webpack", "esbuild", "vite", "rollup", "swc", "turbopack"]
        .iter()
        .any(|&t| base.contains(t))
    {
        return CulpritCategory::BuildTool;
    }

    CulpritCategory::Unknown
}

/// Classify from a ProcessGroup or ProcessInfo (prefers group name).
pub fn classify_culprit_from_blame(
    group: Option<&ProcessGroup>,
    culprit: Option<&ProcessInfo>,
) -> CulpritCategory {
    if let Some(g) = group {
        return classify_culprit(&g.name);
    }
    if let Some(p) = culprit {
        return classify_culprit(&p.cmd);
    }
    CulpritCategory::Unknown
}

// ── Urgency Computation ─────────────────────────────────────────────────────

/// Compute urgency from impact level and trend direction.
pub fn compute_urgency(
    impact: &ImpactLevel,
    cpu_trend: &TrendDirection,
    ram_trend: &TrendDirection,
) -> Urgency {
    let rising = *cpu_trend == TrendDirection::Rising || *ram_trend == TrendDirection::Rising;
    match impact {
        ImpactLevel::Critical => Urgency::ActNow,
        ImpactLevel::Strained => {
            if rising {
                Urgency::ActNow
            } else {
                Urgency::ActSoon
            }
        }
        ImpactLevel::Degrading => {
            if rising {
                Urgency::ActSoon
            } else {
                Urgency::Monitor
            }
        }
        ImpactLevel::Healthy => Urgency::Monitor,
    }
}

// ── Trend Direction from EWMA ───────────────────────────────────────────────

/// Compute trend direction by comparing current value to recent EWMA.
/// `current` is the latest reading, `prev` is the value from the previous tick.
/// Uses a threshold to filter noise.
pub fn compute_trend_direction(current: f64, prev: f64, threshold: f64) -> TrendDirection {
    let delta = current - prev;
    if delta > threshold {
        TrendDirection::Rising
    } else if delta < -threshold {
        TrendDirection::Falling
    } else {
        TrendDirection::Stable
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
    fn test_compute_score_with_io_zero_matches_original() {
        // When io_wait_pct=0, compute_score_with_io should produce same result as compute_score
        let s1 = compute_score(50.0, 50.0, 1.0);
        let s2 = compute_score_with_io(50.0, 50.0, 1.0, 0.0);
        assert!((s1 - s2).abs() < 0.001, "s1={}, s2={}", s1, s2);
    }

    #[test]
    fn test_compute_score_io_dominant_boosts_score() {
        // Low CPU + RAM but high iowait should still produce elevated score
        let without_io = compute_score_with_io(20.0, 20.0, 0.0, 0.0);
        let with_io = compute_score_with_io(20.0, 20.0, 0.0, 80.0);
        assert!(
            with_io > without_io + 0.2,
            "I/O dominant should boost score significantly: {} vs {}",
            with_io,
            without_io
        );
    }

    #[test]
    fn test_compute_score_io_dominant_reaches_degrading() {
        // High iowait alone should reach at least Degrading band (>= 0.20)
        let score = compute_score_with_io(10.0, 10.0, 0.0, 90.0);
        assert!(
            score >= thresholds::IMPACT_LEVEL_HEALTHY_BELOW,
            "I/O dominant score {:.3} should reach degrading band",
            score
        );
    }

    #[test]
    fn test_compute_score_cpu_dominant_reaches_strained() {
        // CPU at 100%, low RAM, no swap — should reach strained band (>= 0.38)
        let score = compute_score(5.0, 100.0, 0.0);
        assert!(
            score >= thresholds::IMPACT_LEVEL_DEGRADING_BELOW,
            "CPU-only saturation score {:.3} should reach strained band (>= {:.2})",
            score,
            thresholds::IMPACT_LEVEL_DEGRADING_BELOW
        );
    }

    #[test]
    fn test_compute_score_ram_dominant_reaches_critical() {
        // RAM at 88% (Critical pressure), low CPU, no swap — should reach critical band (>= 0.55)
        let score = compute_score(88.0, 1.0, 0.0);
        assert!(
            score >= thresholds::IMPACT_LEVEL_STRAINED_BELOW,
            "RAM-only saturation at 88% score {:.3} should reach critical band (>= {:.2})",
            score,
            thresholds::IMPACT_LEVEL_STRAINED_BELOW
        );
    }

    #[test]
    fn test_compute_score_ram_72pct_reaches_strained() {
        // RAM at 72% (memory_pressure threshold), low CPU — should reach strained band (>= 0.38)
        let score = compute_score(72.0, 1.0, 0.0);
        assert!(
            score >= thresholds::IMPACT_LEVEL_DEGRADING_BELOW,
            "RAM at 72% score {:.3} should reach strained band (>= {:.2})",
            score,
            thresholds::IMPACT_LEVEL_DEGRADING_BELOW
        );
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
        assert!(
            fix.contains("unknown_app_xyz") && fix.contains("Close or restart"),
            "expected process-specific fallback, got: {}",
            fix
        );
    }

    #[test]
    fn test_suggest_fix_rustc_maps_to_cargo() {
        let rustc = ProcessInfo {
            pid: 42,
            cmd: "rustc".to_string(),
            cpu_pct: 95.0,
            ram_gb: 1.5,
            blame_score: 0.6,
        };
        let fix = suggest_fix(Some(&rustc), None, &AnomalyType::CpuSaturation);
        assert!(
            fix.contains("cargo build -j 2"),
            "rustc should get cargo build fix, got: {}",
            fix
        );
    }

    // ── Headroom Tests ──────────────────────────────────────────────────

    fn make_hw(
        ram_pressure: RamPressure,
        disk_pressure: DiskPressure,
        throttling: bool,
        cpu: f64,
        temp: Option<f64>,
    ) -> HwSnapshot {
        HwSnapshot {
            die_temp_celsius: temp,
            throttling,
            ram_used_gb: 6.0,
            ram_total_gb: 8.0,
            ram_pressure,
            cpu_usage_pct: cpu,
            disk_used_gb: 400.0,
            disk_total_gb: 500.0,
            disk_pressure,
            headroom: HeadroomLevel::Adequate,
            headroom_reason: String::new(),
            ts: chrono::Utc::now(),
            cpu_trend: TrendDirection::Stable,
            ram_trend: TrendDirection::Stable,
            temp_trend: TrendDirection::Stable,
            cpu_delta_pct: 0.0,
            ram_delta_gb: 0.0,
            top_culprit: String::new(),
            impact_level: ImpactLevel::Healthy,
            impact_duration_s: 0,
            one_liner: String::new(),
            ai_agent_count: 0,
            ai_agent_ram_gb: 0.0,
            swap_used_gb: None,
            swap_total_gb: None,
            disk_fill_rate_gb_per_sec: None,
            irq_per_sec: None,
            system_fd_pct: None,
            oom_freeze_risk: None,
            dot_claude_size_gb: None,
            mcp_server_count: None,
            tmp_claude_size_gb: None,
            process_spawn_rate_per_sec: None,
            net_time_wait_count: None,
            inotify_watch_count: None,
        }
    }

    #[test]
    fn test_headroom_insufficient_ram_critical() {
        let hw = make_hw(
            RamPressure::Critical,
            DiskPressure::Normal,
            false,
            50.0,
            None,
        );
        let (level, reason) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Insufficient);
        assert!(reason.contains("RAM"), "reason: {}", reason);
        assert!(reason.contains("Critical"), "reason: {}", reason);
    }

    #[test]
    fn test_headroom_insufficient_disk_critical() {
        let hw = make_hw(
            RamPressure::Normal,
            DiskPressure::Critical,
            false,
            50.0,
            None,
        );
        let (level, reason) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Insufficient);
        assert!(reason.contains("Disk"), "reason: {}", reason);
    }

    #[test]
    fn test_headroom_insufficient_throttling() {
        let hw = make_hw(
            RamPressure::Normal,
            DiskPressure::Normal,
            true,
            50.0,
            Some(92.0),
        );
        let (level, reason) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Insufficient);
        assert!(reason.contains("throttling"), "reason: {}", reason);
    }

    #[test]
    fn test_headroom_insufficient_warn_plus_high_cpu() {
        let hw = make_hw(RamPressure::Warn, DiskPressure::Normal, false, 75.0, None);
        let (level, _) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Insufficient);
    }

    #[test]
    fn test_headroom_limited_ram_warn() {
        let hw = make_hw(RamPressure::Warn, DiskPressure::Normal, false, 40.0, None);
        let (level, _) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Limited);
    }

    #[test]
    fn test_headroom_limited_high_cpu() {
        let hw = make_hw(RamPressure::Normal, DiskPressure::Normal, false, 72.0, None);
        let (level, _) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Limited);
    }

    #[test]
    fn test_headroom_adequate() {
        let hw = make_hw(RamPressure::Normal, DiskPressure::Normal, false, 30.0, None);
        let (level, reason) = compute_headroom(&hw);
        assert_eq!(level, HeadroomLevel::Adequate);
        assert!(reason.contains("headroom"), "reason: {}", reason);
    }

    #[test]
    fn test_headroom_cpu_boundary() {
        let hw_below = make_hw(RamPressure::Normal, DiskPressure::Normal, false, 69.9, None);
        assert_eq!(compute_headroom(&hw_below).0, HeadroomLevel::Adequate);

        let hw_at = make_hw(RamPressure::Normal, DiskPressure::Normal, false, 70.0, None);
        assert_eq!(compute_headroom(&hw_at).0, HeadroomLevel::Limited);
    }

    #[test]
    fn test_headroom_ram_warn_cpu_boundary() {
        let hw_below = make_hw(RamPressure::Warn, DiskPressure::Normal, false, 69.9, None);
        assert_eq!(compute_headroom(&hw_below).0, HeadroomLevel::Limited);

        let hw_at = make_hw(RamPressure::Warn, DiskPressure::Normal, false, 70.0, None);
        assert_eq!(compute_headroom(&hw_at).0, HeadroomLevel::Insufficient);
    }

    #[test]
    fn test_headroom_disk_warn_cpu_boundary() {
        let hw_below = make_hw(RamPressure::Normal, DiskPressure::Warn, false, 69.9, None);
        assert_eq!(compute_headroom(&hw_below).0, HeadroomLevel::Limited);

        let hw_at = make_hw(RamPressure::Normal, DiskPressure::Warn, false, 70.0, None);
        assert_eq!(compute_headroom(&hw_at).0, HeadroomLevel::Insufficient);

        let hw_high = make_hw(RamPressure::Normal, DiskPressure::Warn, false, 100.0, None);
        let (level, reason) = compute_headroom(&hw_high);
        assert_eq!(level, HeadroomLevel::Insufficient);
        assert!(reason.contains("Disk warn"), "reason: {}", reason);
        assert!(reason.contains("CPU"), "reason: {}", reason);
    }

    // ── Agent Accumulation Tests ─────────────────────────────────────────

    #[test]
    fn test_agent_accumulation_claude() {
        let groups = vec![ProcessGroup {
            name: "claude".to_string(),
            process_count: 3,
            total_cpu_pct: 30.0,
            total_ram_gb: 1.1,
            blame_score: 0.2,
            top_pid: 100,
            pids: vec![100, 101, 102],
        }];
        let result = detect_agent_accumulation(&groups);
        assert!(result.is_some());
        assert_eq!(result.unwrap().process_count, 3);
    }

    #[test]
    fn test_agent_accumulation_cursor() {
        let groups = vec![ProcessGroup {
            name: "Cursor".to_string(),
            process_count: 5,
            total_cpu_pct: 50.0,
            total_ram_gb: 2.0,
            blame_score: 0.3,
            top_pid: 200,
            pids: vec![200, 201, 202, 203, 204],
        }];
        let result = detect_agent_accumulation(&groups);
        assert!(result.is_some());
        assert_eq!(result.unwrap().process_count, 5);
    }

    #[test]
    fn test_agent_accumulation_single_is_normal() {
        let groups = vec![ProcessGroup {
            name: "claude".to_string(),
            process_count: 1,
            total_cpu_pct: 10.0,
            total_ram_gb: 0.3,
            blame_score: 0.1,
            top_pid: 100,
            pids: vec![100],
        }];
        assert!(detect_agent_accumulation(&groups).is_none());
    }

    #[test]
    fn test_agent_accumulation_ignores_non_agents() {
        let groups = vec![ProcessGroup {
            name: "node".to_string(),
            process_count: 10,
            total_cpu_pct: 80.0,
            total_ram_gb: 3.0,
            blame_score: 0.7,
            top_pid: 300,
            pids: vec![300],
        }];
        assert!(detect_agent_accumulation(&groups).is_none());
    }

    #[test]
    fn test_suggest_fix_agent_accumulation() {
        let group = ProcessGroup {
            name: "claude".to_string(),
            process_count: 3,
            total_cpu_pct: 30.0,
            total_ram_gb: 1.1,
            blame_score: 0.2,
            top_pid: 100,
            pids: vec![100, 101, 102],
        };
        let fix = suggest_fix(None, Some(&group), &AnomalyType::AgentAccumulation);
        assert!(fix.contains("3"), "fix: {}", fix);
        assert!(fix.contains("claude"), "fix: {}", fix);
        assert!(fix.contains("1.1GB"), "fix: {}", fix);
    }

    #[test]
    fn test_impact_message_agent_accumulation() {
        let msg = impact_message(&ImpactLevel::Healthy, &AnomalyType::AgentAccumulation);
        assert!(!msg.contains("No action needed"), "msg: {}", msg);
        assert!(msg.contains("agent"), "msg: {}", msg);
    }

    // ── Gap 4: impact lag fix tests ──────────────────────────────────────

    #[test]
    fn test_score_to_level_with_context_extreme_cpu_bypasses_persistence() {
        // At CPU 100%, CUSUM not needed — extreme CPU fast path fires.
        let level = score_to_level_with_context(0.65, false, 100.0, 5.0, 0.0);
        assert_ne!(
            level,
            ImpactLevel::Healthy,
            "extreme CPU bypasses CUSUM requirement"
        );
        assert_eq!(level, ImpactLevel::Critical);
    }

    #[test]
    fn test_score_to_level_with_context_extreme_ram_bypasses_persistence() {
        let level = score_to_level_with_context(0.45, false, 20.0, 90.0, 0.0);
        assert_ne!(level, ImpactLevel::Healthy);
        assert_eq!(level, ImpactLevel::Strained);
    }

    #[test]
    fn test_score_to_level_with_context_normal_load_requires_cusum() {
        // Normal CPU/RAM without CUSUM trigger → Healthy (CUSUM must confirm sustained load).
        let level = score_to_level_with_context(0.35, false, 50.0, 60.0, 0.0);
        assert_eq!(
            level,
            ImpactLevel::Healthy,
            "non-extreme signals require CUSUM confirmation"
        );
        // Same score with CUSUM triggered → Degrading.
        let level2 = score_to_level_with_context(0.35, true, 50.0, 60.0, 0.0);
        assert_eq!(level2, ImpactLevel::Degrading);
    }

    #[test]
    fn test_score_to_level_with_context_cusum_false_without_extreme_signals_is_healthy() {
        // Without CUSUM trigger and without extreme signals, always Healthy
        // regardless of the score (CUSUM must confirm the anomaly first).
        let level = score_to_level_with_context(0.9, false, 50.0, 50.0, 0.0);
        assert_eq!(level, ImpactLevel::Healthy);
    }

    #[test]
    fn test_score_to_level_per_process_cpu_forces_degrading_on_multicore() {
        // On a 4-core machine, one process at 100% = 25% global CPU.
        // Without top_process_cpu_pct, score ≈ 0.125 → always Healthy.
        // With top_process_cpu_pct >= 90, must be at least Degrading.
        let level = score_to_level_with_context(0.125, false, 25.0, 3.0, 100.0);
        assert_eq!(
            level,
            ImpactLevel::Degrading,
            "single-core saturation should force at least Degrading"
        );
    }

    #[test]
    fn test_score_to_level_per_process_high_cpu_does_not_downgrade_critical() {
        // A high system score should not be downgraded by the per-process path
        let level = score_to_level_with_context(0.8, true, 80.0, 70.0, 100.0);
        assert_eq!(level, ImpactLevel::Critical);
    }

    // ── Stranded idle threshold tests ────────────────────────────────────

    #[test]
    fn test_stranded_idle_threshold_constant() {
        // 30 ticks * 2s = 60s — a sub-agent must be idle for a full minute
        assert_eq!(crate::thresholds::AGENT_IDLE_STRANDED_TICKS, 30);
    }

    #[test]
    fn test_stranded_idle_accumulation_logic() {
        // Simulate 30 ticks of idle followed by 1 active tick — should clear
        let mut idle_ticks: std::collections::HashMap<u32, u32> = std::collections::HashMap::new();
        let pid = 9999_u32;

        // 30 idle ticks (just below threshold)
        for _ in 0..30 {
            *idle_ticks.entry(pid).or_insert(0) += 1;
        }
        let stranded: Vec<u32> = idle_ticks
            .iter()
            .filter(|(_, &t)| t >= crate::thresholds::AGENT_IDLE_STRANDED_TICKS)
            .map(|(p, _)| *p)
            .collect();
        assert_eq!(
            stranded,
            vec![pid],
            "30 ticks should reach stranded threshold"
        );

        // One active tick resets the counter
        idle_ticks.insert(pid, 0);
        let stranded_after_reset: Vec<u32> = idle_ticks
            .iter()
            .filter(|(_, &t)| t >= crate::thresholds::AGENT_IDLE_STRANDED_TICKS)
            .map(|(p, _)| *p)
            .collect();
        assert!(
            stranded_after_reset.is_empty(),
            "active tick should clear stranded state"
        );
    }

    // ── Culprit Category Tests ────────────────────────────────────────────

    #[test]
    fn test_classify_culprit_build_tools() {
        assert_eq!(classify_culprit("cargo"), CulpritCategory::BuildTool);
        assert_eq!(classify_culprit("rustc"), CulpritCategory::BuildTool);
        assert_eq!(classify_culprit("gcc"), CulpritCategory::BuildTool);
        assert_eq!(classify_culprit("webpack"), CulpritCategory::BuildTool);
        assert_eq!(classify_culprit("tsc"), CulpritCategory::BuildTool);
    }

    #[test]
    fn test_classify_culprit_browsers() {
        assert_eq!(classify_culprit("Google Chrome"), CulpritCategory::Browser);
        assert_eq!(classify_culprit("firefox"), CulpritCategory::Browser);
        assert_eq!(classify_culprit("safari"), CulpritCategory::Browser);
    }

    #[test]
    fn test_classify_culprit_ides() {
        assert_eq!(classify_culprit("Cursor"), CulpritCategory::Ide);
        assert_eq!(classify_culprit("code"), CulpritCategory::Ide);
        assert_eq!(classify_culprit("zed"), CulpritCategory::Ide);
    }

    #[test]
    fn test_classify_culprit_ai_agents() {
        assert_eq!(classify_culprit("claude"), CulpritCategory::AiAgent);
        assert_eq!(classify_culprit("ollama"), CulpritCategory::AiAgent);
    }

    // ── Gap 6: classify_culprit_from_blame returns AiAgent for claude groups ──

    #[test]
    fn test_classify_culprit_from_blame_claude_group_is_ai_agent() {
        // This is the production path: process_blame culprit_group is a ProcessGroup
        // named "claude". classify_culprit_from_blame must return AiAgent, not Unknown.
        let group = ProcessGroup {
            name: "claude".to_string(),
            process_count: 3,
            total_cpu_pct: 200.0,
            total_ram_gb: 1.5,
            blame_score: 0.8,
            top_pid: 100,
            pids: vec![100, 101, 102],
        };
        assert_eq!(
            classify_culprit_from_blame(Some(&group), None),
            CulpritCategory::AiAgent,
            "a claude ProcessGroup must classify as AiAgent, not Unknown"
        );
    }

    #[test]
    fn test_classify_culprit_from_blame_group_takes_priority_over_culprit() {
        // When both group and culprit are present, the group name wins.
        let group = ProcessGroup {
            name: "claude".to_string(),
            process_count: 2,
            total_cpu_pct: 50.0,
            total_ram_gb: 0.8,
            blame_score: 0.5,
            top_pid: 200,
            pids: vec![200, 201],
        };
        let culprit = ProcessInfo {
            pid: 999,
            cmd: "cargo".to_string(), // would be BuildTool if group were absent
            cpu_pct: 90.0,
            ram_gb: 2.0,
            blame_score: 0.9,
        };
        assert_eq!(
            classify_culprit_from_blame(Some(&group), Some(&culprit)),
            CulpritCategory::AiAgent,
            "group name wins over culprit cmd"
        );
    }

    #[test]
    fn test_classify_culprit_from_blame_no_group_falls_back_to_culprit() {
        let culprit = ProcessInfo {
            pid: 42,
            cmd: "claude".to_string(),
            cpu_pct: 80.0,
            ram_gb: 0.5,
            blame_score: 0.6,
        };
        assert_eq!(
            classify_culprit_from_blame(None, Some(&culprit)),
            CulpritCategory::AiAgent
        );
    }

    #[test]
    fn test_classify_culprit_system() {
        assert_eq!(
            classify_culprit("Memory Compression"),
            CulpritCategory::System
        );
        assert_eq!(classify_culprit("svchost"), CulpritCategory::System);
        assert_eq!(classify_culprit("kernel"), CulpritCategory::System);
    }

    #[test]
    fn test_classify_culprit_unknown() {
        assert_eq!(classify_culprit("myapp"), CulpritCategory::Unknown);
    }

    // ── Urgency Tests ────────────────────────────────────────────────────

    #[test]
    fn test_urgency_critical_always_act_now() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Critical,
                &TrendDirection::Stable,
                &TrendDirection::Stable
            ),
            Urgency::ActNow
        );
    }

    #[test]
    fn test_urgency_strained_rising_is_act_now() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Strained,
                &TrendDirection::Rising,
                &TrendDirection::Stable
            ),
            Urgency::ActNow
        );
    }

    #[test]
    fn test_urgency_strained_stable_is_act_soon() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Strained,
                &TrendDirection::Stable,
                &TrendDirection::Stable
            ),
            Urgency::ActSoon
        );
    }

    #[test]
    fn test_urgency_degrading_rising_is_act_soon() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Degrading,
                &TrendDirection::Rising,
                &TrendDirection::Stable
            ),
            Urgency::ActSoon
        );
    }

    #[test]
    fn test_urgency_degrading_stable_is_monitor() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Degrading,
                &TrendDirection::Stable,
                &TrendDirection::Stable
            ),
            Urgency::Monitor
        );
    }

    #[test]
    fn test_urgency_healthy_is_monitor() {
        assert_eq!(
            compute_urgency(
                &ImpactLevel::Healthy,
                &TrendDirection::Rising,
                &TrendDirection::Rising
            ),
            Urgency::Monitor
        );
    }

    // ── Trend Direction Tests ────────────────────────────────────────────

    #[test]
    fn test_trend_direction_rising() {
        assert_eq!(
            compute_trend_direction(50.0, 40.0, 3.0),
            TrendDirection::Rising
        );
    }

    #[test]
    fn test_trend_direction_falling() {
        assert_eq!(
            compute_trend_direction(30.0, 40.0, 3.0),
            TrendDirection::Falling
        );
    }

    #[test]
    fn test_trend_direction_stable_within_threshold() {
        assert_eq!(
            compute_trend_direction(41.0, 40.0, 3.0),
            TrendDirection::Stable
        );
    }

    /// Cursor-specific fix across all anomaly types: always suggests Cmd+W.
    #[test]
    fn test_suggest_fix_cursor_all_anomaly_types() {
        let culprit = ProcessInfo {
            pid: 55600,
            cmd: "Cursor Helper (Renderer)".to_string(),
            cpu_pct: 80.0,
            ram_gb: 0.5,
            blame_score: 0.4,
        };
        let group = ProcessGroup {
            name: "Cursor".to_string(),
            process_count: 1,
            total_cpu_pct: 80.0,
            total_ram_gb: 0.5,
            blame_score: 0.4,
            top_pid: 55600,
            pids: vec![55600],
        };

        for anomaly in &[
            AnomalyType::CpuSaturation,
            AnomalyType::MemoryPressure,
            AnomalyType::ThermalThrottle,
            AnomalyType::GeneralSlowdown,
            AnomalyType::None,
        ] {
            let fix = suggest_fix(Some(&culprit), Some(&group), anomaly);
            assert!(
                fix.contains("Cursor") && fix.contains("Cmd+W"),
                "anomaly {:?}: fix should mention Cursor and Cmd+W, got: {}",
                anomaly,
                fix
            );
        }
    }

    /// Cursor with many helpers should still get the Cursor-specific fix, not generic.
    #[test]
    fn test_suggest_fix_cursor_multi_process_group() {
        let group = ProcessGroup {
            name: "Cursor".to_string(),
            process_count: 20,
            total_cpu_pct: 50.0,
            total_ram_gb: 2.0,
            blame_score: 0.3,
            top_pid: 55600,
            pids: (100..120).collect(),
        };
        let fix = suggest_fix(None, Some(&group), &AnomalyType::MemoryPressure);
        assert!(
            fix.contains("Cursor") && fix.contains("Cmd+W"),
            "20-process Cursor group should get Cursor-specific fix, got: {}",
            fix
        );
    }

    #[test]
    fn test_suggest_fix_axon_multiple_instances() {
        let self_pid = std::process::id();
        let group = ProcessGroup {
            name: "axon".to_string(),
            process_count: 3,
            total_cpu_pct: 250.0,
            total_ram_gb: 0.1,
            blame_score: 0.5,
            top_pid: 9999,
            pids: vec![self_pid, 9999, 8888],
        };
        let fix = suggest_fix(None, Some(&group), &AnomalyType::CpuSaturation);
        // Should mention stale instances and include kill command with non-self PIDs
        assert!(fix.contains("stale axon"), "fix: {}", fix);
        assert!(fix.contains("kill"), "fix: {}", fix);
        assert!(fix.contains("9999"), "fix: {}", fix);
        assert!(fix.contains("8888"), "fix: {}", fix);
        // Should NOT include self PID
        assert!(
            !fix.contains(&self_pid.to_string()),
            "fix should not contain self PID: {}",
            fix
        );
    }

    #[test]
    fn test_suggest_fix_axon_single_instance_falls_through() {
        let group = ProcessGroup {
            name: "axon".to_string(),
            process_count: 1,
            total_cpu_pct: 50.0,
            total_ram_gb: 0.05,
            blame_score: 0.3,
            top_pid: std::process::id(),
            pids: vec![std::process::id()],
        };
        let fix = suggest_fix(None, Some(&group), &AnomalyType::CpuSaturation);
        // Single instance should fall through to generic fix, not mention stale
        assert!(!fix.contains("stale axon"), "fix: {}", fix);
    }
}
