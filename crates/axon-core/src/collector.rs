use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use sysinfo::{Disks, System};
use tokio::time::{interval, Duration};
use tracing::debug;

use crate::{
    alerts::{self, AlertContext},
    ewma::EwmaStore,
    gpu, grouping, impact, persistence,
    ring_buffer::SnapshotRing,
    temperature,
    types::*,
};

struct TestPrevStateConfig {
    ram_pressure: Option<RamPressure>,
    throttling: Option<bool>,
    impact_level: Option<ImpactLevel>,
    disk_pressure: Option<DiskPressure>,
    preserve_during_warmup: bool,
}

impl TestPrevStateConfig {
    fn from_env() -> Self {
        Self {
            ram_pressure: std::env::var("AXON_TEST_PREV_RAM_PRESSURE")
                .ok()
                .and_then(|v| parse_ram_pressure(&v)),
            throttling: std::env::var("AXON_TEST_PREV_THROTTLING")
                .ok()
                .and_then(|v| parse_bool(&v)),
            impact_level: std::env::var("AXON_TEST_PREV_IMPACT_LEVEL")
                .ok()
                .and_then(|v| parse_impact_level(&v)),
            disk_pressure: std::env::var("AXON_TEST_PREV_DISK_PRESSURE")
                .ok()
                .and_then(|v| parse_disk_pressure(&v)),
            preserve_during_warmup: std::env::var("AXON_TEST_PRESERVE_PREV_DURING_WARMUP")
                .ok()
                .and_then(|v| parse_bool(&v))
                .unwrap_or(false),
        }
    }
}

fn parse_bool(s: &str) -> Option<bool> {
    match s.trim().to_ascii_lowercase().as_str() {
        "1" | "true" | "yes" | "y" | "on" => Some(true),
        "0" | "false" | "no" | "n" | "off" => Some(false),
        _ => None,
    }
}

fn parse_ram_pressure(s: &str) -> Option<RamPressure> {
    match s.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(RamPressure::Normal),
        "warn" | "warning" => Some(RamPressure::Warn),
        "critical" => Some(RamPressure::Critical),
        _ => None,
    }
}

fn parse_impact_level(s: &str) -> Option<ImpactLevel> {
    match s.trim().to_ascii_lowercase().as_str() {
        "healthy" => Some(ImpactLevel::Healthy),
        "degrading" => Some(ImpactLevel::Degrading),
        "strained" => Some(ImpactLevel::Strained),
        "critical" => Some(ImpactLevel::Critical),
        _ => None,
    }
}

fn parse_disk_pressure(s: &str) -> Option<DiskPressure> {
    match s.trim().to_ascii_lowercase().as_str() {
        "normal" => Some(DiskPressure::Normal),
        "warn" | "warning" => Some(DiskPressure::Warn),
        "critical" => Some(DiskPressure::Critical),
        _ => None,
    }
}

// ── Shared Application State ──────────────────────────────────────────────────

pub struct AppState {
    pub hw: HwSnapshot,
    pub blame: ProcessBlame,
    pub battery: Option<BatteryStatus>,
    pub profile: SystemProfile,
    pub processes: Vec<ProcessInfo>,
    pub groups: Vec<ProcessGroup>,
    pub pending_alerts: Vec<Alert>,
    pub gpu: Option<GpuSnapshot>,
}

impl AppState {
    pub fn new(profile: SystemProfile) -> Self {
        let now = chrono::Utc::now();
        Self {
            hw: HwSnapshot {
                die_temp_celsius: None,
                throttling: false,
                ram_used_gb: 0.0,
                ram_total_gb: 0.0,
                ram_pressure: RamPressure::Normal,
                cpu_usage_pct: 0.0,
                disk_used_gb: 0.0,
                disk_total_gb: 0.0,
                disk_pressure: DiskPressure::Normal,
                headroom: HeadroomLevel::Adequate,
                headroom_reason: "System has headroom".to_string(),
                ts: now,
            },
            blame: ProcessBlame {
                anomaly_type: AnomalyType::None,
                impact_level: ImpactLevel::Healthy,
                culprit: None,
                culprit_group: None,
                anomaly_score: 0.0,
                impact: "System is healthy. No action needed.".to_string(),
                fix: "No action needed.".to_string(),
                ts: now,
                stale_axon_pids: Vec::new(),
            },
            battery: None,
            profile,
            processes: Vec::new(),
            groups: Vec::new(),
            pending_alerts: Vec::new(),
            gpu: None,
        }
    }
}

pub type SharedState = Arc<Mutex<AppState>>;

// ── Collector Loop ────────────────────────────────────────────────────────────

/// Tracks per-metric debounce and flap detection state.
struct DebounceState {
    /// Consecutive ticks that RAM pressure has been at a different level from confirmed.
    ram_ticks: u32,
    /// Consecutive ticks that disk pressure has been at a different level from confirmed.
    disk_ticks: u32,
    /// Consecutive ticks that CPU saturation has been at a different boolean from confirmed.
    cpu_sat_ticks: u32,
    /// Consecutive ticks that throttling has been at a different boolean from confirmed.
    throttle_ticks: u32,
    /// Ring of recent RAM-pressure boundary crossings for flap detection (tick numbers).
    ram_crossings: Vec<u32>,
    /// Ring of recent disk-pressure boundary crossings for flap detection (tick numbers).
    disk_crossings: Vec<u32>,
}

impl DebounceState {
    fn new() -> Self {
        Self {
            ram_ticks: 0,
            disk_ticks: 0,
            cpu_sat_ticks: 0,
            throttle_ticks: 0,
            ram_crossings: Vec::new(),
            disk_crossings: Vec::new(),
        }
    }

    /// Record a boundary crossing at the given tick and prune old entries.
    fn record_crossing(crossings: &mut Vec<u32>, tick: u32) {
        crossings.push(tick);
        let cutoff = tick.saturating_sub(crate::thresholds::FLAP_WINDOW_TICKS);
        crossings.retain(|&t| t >= cutoff);
    }

    /// True if the number of crossings in the flap window exceeds the threshold.
    fn is_flapping(crossings: &[u32]) -> bool {
        crossings.len() as u32 > crate::thresholds::FLAP_THRESHOLD
    }
}

/// Spawns a background Tokio task that refreshes hardware state every 2 seconds.
/// Updates the SharedState in place for the MCP server to read.
pub async fn start_collector(state: SharedState, db: persistence::DbHandle, ring: SnapshotRing) {
    let mut sys = System::new_all();
    let mut ewma = EwmaStore::default();
    let mut tick_count: u32 = 0;
    let mut above_threshold_count: u32 = 0;
    let test_prev = TestPrevStateConfig::from_env();

    // Confirmed (debounced) state for transition detection
    let mut prev_ram_pressure = test_prev.ram_pressure.unwrap_or(RamPressure::Normal);
    let mut prev_throttling = test_prev.throttling.unwrap_or(false);
    let mut prev_impact_level = test_prev.impact_level.unwrap_or(ImpactLevel::Healthy);
    let mut prev_disk_pressure = test_prev.disk_pressure.unwrap_or(DiskPressure::Normal);
    let mut prev_cpu_saturated = false;

    // Debounce / flap detection
    let mut debounce = DebounceState::new();

    // Rate limiting: last tick each alert type fired
    let mut last_alert_tick: HashMap<AlertType, u32> = HashMap::new();

    let self_pid = std::process::id();

    let mut ticker = interval(Duration::from_secs(2));

    loop {
        ticker.tick().await;
        tick_count += 1;

        // Refresh sysinfo (blocking ~5ms on M-series)
        sys.refresh_all();

        // ── Hardware snapshot ──────────────────────────────────────────────

        let cpu_pct = sys.global_cpu_usage() as f64;
        let total_mem = sys.total_memory(); // bytes
        let used_mem = sys.used_memory(); // bytes
        let ram_total_gb = total_mem as f64 / 1_073_741_824.0;
        let ram_used_gb = used_mem as f64 / 1_073_741_824.0;
        let ram_pct = if total_mem > 0 {
            used_mem as f64 / total_mem as f64 * 100.0
        } else {
            0.0
        };

        let ram_pressure =
            crate::thresholds::ram_pressure_with_hysteresis(ram_pct, &prev_ram_pressure);

        let die_temp = temperature::read_cpu_temp();
        let throttling =
            crate::thresholds::thermal_throttling_with_hysteresis(die_temp, prev_throttling);

        // ── Disk space (root volume) ──────────────────────────────────────
        let disks = Disks::new_with_refreshed_list();
        let (disk_total_gb, disk_used_gb) = disks
            .iter()
            .find(|d| {
                let mp = d.mount_point();
                mp == std::path::Path::new("/") || mp == std::path::Path::new("C:\\")
            })
            .map(|d| {
                let total = d.total_space() as f64 / 1_073_741_824.0;
                let avail = d.available_space() as f64 / 1_073_741_824.0;
                (total, total - avail)
            })
            .unwrap_or((0.0, 0.0));

        let disk_pct = if disk_total_gb > 0.0 {
            disk_used_gb / disk_total_gb * 100.0
        } else {
            0.0
        };
        let disk_pressure =
            crate::thresholds::disk_pressure_with_hysteresis(disk_pct, &prev_disk_pressure);

        let mut hw = HwSnapshot {
            die_temp_celsius: die_temp,
            throttling,
            ram_used_gb,
            ram_total_gb,
            ram_pressure: ram_pressure.clone(),
            cpu_usage_pct: cpu_pct,
            disk_used_gb,
            disk_total_gb,
            disk_pressure: disk_pressure.clone(),
            headroom: HeadroomLevel::Adequate,
            headroom_reason: String::new(),
            ts: chrono::Utc::now(),
        };
        let (headroom, headroom_reason) = impact::compute_headroom(&hw);
        hw.headroom = headroom;
        hw.headroom_reason = headroom_reason;

        // ── Process collection + EWMA update ──────────────────────────────

        let cpu_count = sys.cpus().len().max(1) as f64;
        let mut process_infos: Vec<ProcessInfo> = Vec::new();
        let active_pids: Vec<u32> = sys
            .processes()
            .keys()
            .map(|p| usize::from(*p) as u32)
            .collect();

        let anomaly_type = impact::detect_anomaly_type(ram_pct, cpu_pct, die_temp);

        for (pid, process) in sys.processes() {
            // cpu_usage from sysinfo can exceed 100% on multi-core (e.g., 400% = 4 cores)
            let raw_cpu = process.cpu_usage() as f64;
            let cpu_normalised = raw_cpu / cpu_count; // normalise to 0-100% relative to all CPUs
            let ram_bytes = process.memory();
            let ram_gb = ram_bytes as f64 / 1_073_741_824.0;
            let pid_u32 = usize::from(*pid) as u32;

            let (cpu_delta, ram_delta) = ewma.update(pid_u32, cpu_normalised, ram_gb);

            // Compute blame score weighted by anomaly type.
            // When EWMA hasn't warmed up (delta = 0,0), fall back to raw
            // values so that a process at 100% CPU always outranks one at 1%.
            let using_raw = cpu_delta == 0.0 && ram_delta == 0.0;
            let (eff_cpu, eff_ram) = if using_raw {
                (cpu_normalised, ram_gb)
            } else {
                (cpu_delta, ram_delta)
            };

            let blame_score = match anomaly_type {
                AnomalyType::ThermalThrottle | AnomalyType::CpuSaturation => {
                    0.6 * (eff_cpu / 100.0).min(1.0)
                        + 0.4 * (eff_ram / ram_total_gb.max(1.0)).min(1.0)
                }
                AnomalyType::MemoryPressure => {
                    0.25 * (eff_cpu / 100.0).min(1.0)
                        + 0.75 * (eff_ram / ram_total_gb.max(1.0)).min(1.0)
                }
                _ => {
                    0.5 * (eff_cpu / 100.0).min(1.0)
                        + 0.5 * (eff_ram / ram_total_gb.max(1.0)).min(1.0)
                }
            };

            // Only track processes that are actually interesting
            if blame_score > 0.02 || raw_cpu > 5.0 || ram_gb > 0.2 {
                let cmd = process.name().to_string_lossy().into_owned();
                process_infos.push(ProcessInfo {
                    pid: pid_u32,
                    cmd,
                    cpu_pct: raw_cpu,
                    ram_gb,
                    blame_score,
                });
            }
        }

        // Detect sibling axon serve instances (not self) before excluding self
        let stale_axon_pids: Vec<u32> = process_infos
            .iter()
            .filter(|p| {
                p.pid != self_pid
                    && grouping::normalize_process_name(&p.cmd)
                        .to_lowercase()
                        .contains("axon")
            })
            .map(|p| p.pid)
            .collect();

        // Exclude self from blame — we don't want axon blaming itself
        process_infos.retain(|p| p.pid != self_pid);

        // Sort by blame score descending — top culprit is first
        process_infos.sort_by(|a, b| {
            b.blame_score
                .partial_cmp(&a.blame_score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });

        // Cleanup stale EWMA entries
        ewma.cleanup(&active_pids);

        // ── System anomaly scoring + persistence ───────────────────────────

        let swap_gb = sys.used_swap() as f64 / 1_073_741_824.0;
        let io_wait_pct = impact::read_io_wait_pct();
        let score = impact::compute_score_with_io(ram_pct, cpu_pct, swap_gb, io_wait_pct);

        if score > crate::thresholds::IMPACT_SCORE_ELEVATED {
            above_threshold_count = above_threshold_count.saturating_add(1);
        } else {
            above_threshold_count = above_threshold_count.saturating_sub(1);
        }

        let impact_level = impact::score_to_level(score, above_threshold_count);
        let culprit = process_infos.first().cloned();
        let groups = grouping::build_groups(&process_infos);

        // Check for agent accumulation — only override anomaly_type when the
        // agent group is actually the top blame group (highest blame_score).
        // Otherwise a 2-process claude sitting idle would mask a `yes` process
        // burning 400% CPU.
        let (anomaly_type, culprit_group) =
            if let Some(agent_group) = impact::detect_agent_accumulation(&groups) {
                let top_group = groups.first();
                let agents_are_top = top_group
                    .map(|g| g.name == agent_group.name)
                    .unwrap_or(false);
                // Also check raw CPU: if any non-agent group uses 3x+ more CPU
                // than agents, they're the real hog (EWMA may have stabilized
                // on the hog, flattening its blame_score).
                let non_agent_cpu_hog = groups.iter().find(|g| {
                    g.name != agent_group.name
                        && g.total_cpu_pct > agent_group.total_cpu_pct * 3.0
                });
                if agents_are_top && non_agent_cpu_hog.is_none() {
                    (AnomalyType::AgentAccumulation, Some(agent_group.clone()))
                } else if let Some(hog) = non_agent_cpu_hog {
                    (anomaly_type, Some(hog.clone()))
                } else if !agents_are_top {
                    (anomaly_type, top_group.cloned())
                } else {
                    (AnomalyType::AgentAccumulation, Some(agent_group.clone()))
                }
            } else {
                (anomaly_type, groups.first().cloned())
            };

        let impact_msg = impact::impact_message(&impact_level, &anomaly_type);
        let fix = impact::suggest_fix(culprit.as_ref(), culprit_group.as_ref(), &anomaly_type);

        let blame = ProcessBlame {
            anomaly_type,
            impact_level: impact_level.clone(),
            culprit,
            culprit_group,
            anomaly_score: score,
            impact: impact_msg,
            fix,
            ts: chrono::Utc::now(),
            stale_axon_pids,
        };

        debug!(
            tick = tick_count,
            cpu = %format!("{:.0}%", cpu_pct),
            ram = %format!("{:.1}/{:.0}GB", ram_used_gb, ram_total_gb),
            score = %format!("{:.2}", score),
            "tick"
        );

        // ── GPU snapshot (every tick) ─────────────────────────────────────

        // Always store the snapshot so the MCP tool can report "no GPU detected"
        // rather than appearing to have no data at all.
        let gpu = Some(gpu::read_gpu_snapshot());

        // ── Battery (every 15 ticks ≈ 30s) ────────────────────────────────

        let battery = if tick_count % 15 == 1 {
            read_battery()
        } else {
            // Reuse existing value without locking
            let guard = state.lock().unwrap();
            guard.battery.clone()
        };

        // ── Alert generation (state transitions with debounce + flap detection) ─

        // Skip alerts during warm-up (first 3 ticks)
        let mut new_alerts = if tick_count > 3 {
            let cpu_saturated = cpu_pct > crate::thresholds::ANOMALY_CPU_PCT;
            let debounce_n = crate::thresholds::ALERT_DEBOUNCE_TICKS;

            // ── Debounce RAM pressure ───────────────────────────────
            let debounced_ram = if ram_pressure != prev_ram_pressure {
                debounce.ram_ticks += 1;
                if debounce.ram_ticks >= debounce_n {
                    DebounceState::record_crossing(&mut debounce.ram_crossings, tick_count);
                    debounce.ram_ticks = 0;
                    true // transition confirmed
                } else {
                    false
                }
            } else {
                debounce.ram_ticks = 0;
                false
            };

            // ── Debounce disk pressure ──────────────────────────────
            let debounced_disk = if disk_pressure != prev_disk_pressure {
                debounce.disk_ticks += 1;
                if debounce.disk_ticks >= debounce_n {
                    DebounceState::record_crossing(&mut debounce.disk_crossings, tick_count);
                    debounce.disk_ticks = 0;
                    true
                } else {
                    false
                }
            } else {
                debounce.disk_ticks = 0;
                false
            };

            // ── Debounce CPU saturation ─────────────────────────────
            let debounced_cpu = if cpu_saturated != prev_cpu_saturated {
                debounce.cpu_sat_ticks += 1;
                debounce.cpu_sat_ticks >= debounce_n
            } else {
                debounce.cpu_sat_ticks = 0;
                false
            };
            if debounced_cpu {
                debounce.cpu_sat_ticks = 0;
            }

            // ── Debounce thermal throttling ─────────────────────────
            let debounced_throttle = if throttling != prev_throttling {
                debounce.throttle_ticks += 1;
                debounce.throttle_ticks >= debounce_n
            } else {
                debounce.throttle_ticks = 0;
                false
            };
            if debounced_throttle {
                debounce.throttle_ticks = 0;
            }

            // Suppress alerts if flapping is detected
            let ram_flapping = DebounceState::is_flapping(&debounce.ram_crossings);
            let disk_flapping = DebounceState::is_flapping(&debounce.disk_crossings);

            if ram_flapping {
                debug!(tick = tick_count, "RAM pressure flapping detected, suppressing alerts");
            }
            if disk_flapping {
                debug!(tick = tick_count, "Disk pressure flapping detected, suppressing alerts");
            }

            // Build alert context with debounced transitions.
            // For debounced metrics: use prev as "before" and current as "after" only if
            // the debounce confirmed. Otherwise, present them as unchanged to prevent alerts.
            let alert_ram_prev = if debounced_ram && !ram_flapping {
                &prev_ram_pressure
            } else {
                &ram_pressure // same as current → no transition detected
            };
            let alert_disk_prev = if debounced_disk && !disk_flapping {
                &prev_disk_pressure
            } else {
                &disk_pressure
            };
            let alert_cpu_prev = if debounced_cpu { prev_cpu_saturated } else { cpu_saturated };
            let alert_throttle_prev = if debounced_throttle { prev_throttling } else { throttling };

            let ctx = AlertContext {
                prev_ram_pressure: alert_ram_prev,
                ram_pressure: &ram_pressure,
                prev_throttling: alert_throttle_prev,
                throttling,
                die_temp,
                ram_used_gb,
                ram_total_gb,
                cpu_pct,
                prev_cpu_saturated: alert_cpu_prev,
                cpu_saturated,
                prev_disk_pressure: alert_disk_prev,
                disk_pressure: &disk_pressure,
                disk_used_gb,
                disk_total_gb,
                prev_impact_level: &prev_impact_level,
                impact_level: &impact_level,
                impact_message: &blame.impact,
                culprit: blame.culprit.as_ref(),
                culprit_group: blame.culprit_group.as_ref(),
            };
            let a = alerts::detect_alerts(&ctx);

            // Update confirmed state only when debounce confirms the transition
            if debounced_ram {
                prev_ram_pressure = ram_pressure;
            }
            if debounced_throttle {
                prev_throttling = throttling;
            }
            // Impact level is not debounced (already has its own persistence mechanism)
            prev_impact_level = impact_level;
            if debounced_disk {
                prev_disk_pressure = disk_pressure;
            }
            if debounced_cpu {
                prev_cpu_saturated = cpu_saturated;
            }
            a
        } else {
            // Optional test hook: preserve injected previous-state values through warm-up so
            // the first alert-enabled tick can deterministically validate edge transitions.
            if !test_prev.preserve_during_warmup {
                prev_ram_pressure = ram_pressure;
                prev_throttling = throttling;
                prev_impact_level = impact_level;
                prev_disk_pressure = disk_pressure;
                prev_cpu_saturated = cpu_pct > crate::thresholds::ANOMALY_CPU_PCT;
            }
            Vec::new()
        };

        // ── Rate limit: suppress same-type alerts within cooldown window ──
        let rate_limit = crate::thresholds::ALERT_RATE_LIMIT_TICKS;
        new_alerts.retain(|alert| {
            // Recovery alerts bypass rate limiting
            if alert.severity == AlertSeverity::Resolved {
                return true;
            }
            let last = last_alert_tick.get(&alert.alert_type).copied().unwrap_or(0);
            if tick_count.saturating_sub(last) >= rate_limit || last == 0 {
                last_alert_tick.insert(alert.alert_type.clone(), tick_count);
                true
            } else {
                debug!(
                    tick = tick_count,
                    alert_type = %alert.alert_type,
                    "alert rate-limited (last fired at tick {})", last
                );
                false
            }
        });

        // ── Persist alerts immediately (independent of MCP connection) ───
        // Alerts are persisted here so they land in the DB even when there is no active
        // MCP client (e.g. test harnesses, or short-lived connections). The alert_sender
        // task handles webhook dispatch and MCP logging notifications separately.
        for alert in &new_alerts {
            persistence::insert_alert(&db, alert);
        }

        // ── Write to shared state ──────────────────────────────────────────

        let mut guard = state.lock().unwrap();
        guard.hw = hw.clone();
        guard.blame = blame.clone();
        guard.battery = battery;
        guard.processes = process_infos;
        guard.groups = groups;
        guard.pending_alerts.append(&mut new_alerts);
        guard.gpu = gpu;
        drop(guard);

        // ── Push to ring buffer every tick (full 2s resolution) ────────
        ring.push(hw.clone());

        // ── Persist snapshot every 5 ticks (~10s) ────────────────────────

        if tick_count == 1 || tick_count.is_multiple_of(5) {
            persistence::insert_snapshot(&db, &hw, &blame);
        }
    }
}

// ── Battery Reader ────────────────────────────────────────────────────────────

fn read_battery() -> Option<BatteryStatus> {
    #[cfg(target_os = "macos")]
    {
        read_battery_macos()
    }
    #[cfg(target_os = "linux")]
    {
        read_battery_linux()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        None
    }
}

#[cfg(target_os = "macos")]
fn read_battery_macos() -> Option<BatteryStatus> {
    use std::process::Command;

    let output = Command::new("pmset").args(["-g", "batt"]).output().ok()?;
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    if stdout.is_empty() {
        return None;
    }

    let is_charging = stdout.contains("AC Power") || stdout.contains("charging");

    let percentage: f32 = stdout.split('%').next().and_then(|s| {
        s.split(|c: char| c.is_whitespace() || c == '\t' || c == ';')
            .rfind(|sub| !sub.is_empty())
            .and_then(|sub| sub.trim().parse().ok())
    })?;

    let time_to_empty = if !is_charging {
        stdout.lines().find_map(|line| {
            if line.contains("remaining")
                && !line.contains("no estimate")
                && !line.contains("-1:-1")
            {
                line.split_whitespace()
                    .find(|s| s.contains(':') && !s.starts_with('-'))
                    .and_then(|t| {
                        let parts: Vec<&str> = t.split(':').collect();
                        if parts.len() == 2 {
                            let h: u32 = parts[0].parse().ok()?;
                            let m: u32 = parts[1].parse().ok()?;
                            Some(h * 60 + m)
                        } else {
                            None
                        }
                    })
            } else {
                None
            }
        })
    } else {
        None
    };

    Some(build_battery_status(percentage, is_charging, time_to_empty))
}

#[cfg(target_os = "linux")]
fn read_battery_linux() -> Option<BatteryStatus> {
    // Read from /sys/class/power_supply/BAT0 (or BAT1)
    let bat_dir = if std::path::Path::new("/sys/class/power_supply/BAT0/capacity").exists() {
        "/sys/class/power_supply/BAT0"
    } else if std::path::Path::new("/sys/class/power_supply/BAT1/capacity").exists() {
        "/sys/class/power_supply/BAT1"
    } else {
        return None;
    };

    let percentage: f32 = std::fs::read_to_string(format!("{}/capacity", bat_dir))
        .ok()?
        .trim()
        .parse()
        .ok()?;

    let status = std::fs::read_to_string(format!("{}/status", bat_dir))
        .ok()
        .map(|s| s.trim().to_string())
        .unwrap_or_default();
    let is_charging = status == "Charging" || status == "Full";

    // Try to compute time remaining from energy/power
    let time_to_empty = if !is_charging {
        let energy_now: f64 = std::fs::read_to_string(format!("{}/energy_now", bat_dir))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.0);
        let power_now: f64 = std::fs::read_to_string(format!("{}/power_now", bat_dir))
            .ok()
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or(0.0);
        if power_now > 0.0 {
            Some((energy_now / power_now * 60.0) as u32)
        } else {
            None
        }
    } else {
        None
    };

    Some(build_battery_status(percentage, is_charging, time_to_empty))
}

fn build_battery_status(
    percentage: f32,
    is_charging: bool,
    time_to_empty: Option<u32>,
) -> BatteryStatus {
    let narrative = if is_charging {
        format!("Battery at {:.0}% and charging.", percentage)
    } else if let Some(mins) = time_to_empty {
        let h = mins / 60;
        let m = mins % 60;
        if h > 0 {
            format!("Battery at {:.0}% (~{}h {}m remaining).", percentage, h, m)
        } else {
            format!("Battery at {:.0}% (~{}m remaining).", percentage, m)
        }
    } else {
        format!("Battery at {:.0}% (estimating remaining time).", percentage)
    };

    BatteryStatus {
        percentage,
        is_charging,
        time_to_empty_min: time_to_empty,
        narrative,
    }
}

// ── System Profile (built once at startup) ────────────────────────────────────

pub fn build_system_profile() -> SystemProfile {
    let sys = System::new_all();

    let os_version = System::long_os_version().unwrap_or_else(|| "Unknown".to_string());
    let core_count = sys.cpus().len();
    let ram_total_gb = sys.total_memory() as f64 / 1_073_741_824.0;

    let (model_id, chip) = detect_platform_info(&sys);

    // Detect sibling axon serve instances at startup
    let self_pid = std::process::id();
    let axon_siblings: Vec<u32> = sys
        .processes()
        .iter()
        .filter(|(pid, p)| {
            let pid_u32 = usize::from(**pid) as u32;
            pid_u32 != self_pid
                && p.name().to_string_lossy().to_lowercase().contains("axon")
                && p.cmd().iter().any(|arg| arg.to_string_lossy().contains("serve"))
        })
        .map(|(pid, _)| usize::from(*pid) as u32)
        .collect();

    let mut startup_warnings = Vec::new();
    if !axon_siblings.is_empty() {
        let pid_list = axon_siblings
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        startup_warnings.push(format!(
            "{} stale axon serve instance(s) detected (PIDs: {}). Kill them: kill {}",
            axon_siblings.len(),
            pid_list,
            pid_list,
        ));
    }

    SystemProfile {
        model_id,
        chip,
        core_count,
        ram_total_gb,
        os_version,
        axon_version: VERSION.to_string(),
        startup_warnings,
    }
}

#[cfg(target_os = "macos")]
fn detect_platform_info(_sys: &System) -> (String, String) {
    use std::process::Command;

    let model_id = Command::new("sysctl")
        .args(["-n", "hw.model"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|| "Unknown".to_string());

    // On Apple Silicon, machdep.cpu.brand_string may be absent; fall back gracefully
    let chip = Command::new("sysctl")
        .args(["-n", "machdep.cpu.brand_string"])
        .output()
        .ok()
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            Command::new("sysctl")
                .args(["-n", "hw.perflevel0.name"])
                .output()
                .ok()
                .and_then(|o| String::from_utf8(o.stdout).ok())
                .map(|s| format!("Apple Silicon ({})", s.trim()))
                .unwrap_or_else(|| "Apple Silicon".to_string())
        });

    (model_id, chip)
}

#[cfg(target_os = "linux")]
fn detect_platform_info(sys: &System) -> (String, String) {
    // CPU model from sysinfo (reads /proc/cpuinfo internally)
    let chip = sys
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    // Model ID: try DMI product name, fall back to hostname or "Linux Machine"
    let model_id = std::fs::read_to_string("/sys/devices/virtual/dmi/id/product_name")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && s != "None" && s != "To Be Filled By O.E.M.")
        .or_else(|| {
            std::fs::read_to_string("/sys/devices/virtual/dmi/id/board_name")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty() && s != "None")
        })
        .unwrap_or_else(|| {
            sysinfo::System::host_name().unwrap_or_else(|| "Linux Machine".to_string())
        });

    (model_id, chip)
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
fn detect_platform_info(sys: &System) -> (String, String) {
    let chip = sys
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());
    let model_id = sysinfo::System::host_name().unwrap_or_else(|| "Unknown Machine".to_string());
    (model_id, chip)
}
