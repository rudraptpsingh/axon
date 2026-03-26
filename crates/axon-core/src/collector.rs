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

// ── CUSUM state for system-level impact scoring ───────────────────────────────
// Replaces the old `above_threshold_count` counter. CUSUM (Cumulative Sum,
// Page 1954) is optimal for detecting step changes: it accumulates evidence
// that the score has shifted above the idle baseline, and does NOT re-adapt
// to sustained anomalies the way EWMA does (no "inertia problem").
//
// Initialisation uses Lucas & Crosier (1982) FIR headstart (s_pos = h/2) so
// the detector responds faster to an anomaly that exists at startup.
// The reference mean μ is estimated from the first CUSUM_WARMUP_TICKS ticks.

const CUSUM_WARMUP_TICKS: usize = 3;
/// Fixed noise floor for normalised score (empirically ~0.04 on an idle machine).
const CUSUM_SIGMA: f64 = 0.04;
/// Allowance: detect shifts > 0.5σ above μ (standard k=0.5 default).
const CUSUM_K: f64 = 0.5;
/// Alert threshold (4σ → ~1 false alarm per 370 ticks ≈ 12 min at 2s ticks).
const CUSUM_H: f64 = 4.0;
/// After this many consecutive alert ticks (~60s), slowly adapt μ to acknowledge
/// a genuine new baseline (e.g. user started a long compile job).
const CUSUM_ADAPT_TICKS: u32 = 30;

struct CusumState {
    s_pos: f64,
    mu: f64,
    warmup_scores: Vec<f64>,
    ticks_since_alert: u32,
}

impl CusumState {
    fn new() -> Self {
        Self {
            s_pos: 0.0,
            mu: 0.0,
            warmup_scores: Vec::with_capacity(CUSUM_WARMUP_TICKS),
            ticks_since_alert: 0,
        }
    }

    /// Feed a new score. Returns true when CUSUM signals a sustained anomaly.
    fn update(&mut self, score: f64) -> bool {
        // Collect first N scores to estimate the idle baseline μ.
        if self.warmup_scores.len() < CUSUM_WARMUP_TICKS {
            self.warmup_scores.push(score);
            if self.warmup_scores.len() == CUSUM_WARMUP_TICKS {
                self.mu = self.warmup_scores.iter().sum::<f64>() / CUSUM_WARMUP_TICKS as f64;
                // FIR headstart: begin at h/2 so startup anomalies are caught faster.
                self.s_pos = CUSUM_H / 2.0;
            }
            return false;
        }

        let z = (score - self.mu) / CUSUM_SIGMA;
        self.s_pos = (self.s_pos + z - CUSUM_K).max(0.0);

        let triggered = self.s_pos >= CUSUM_H;
        if triggered {
            self.ticks_since_alert += 1;
            if self.ticks_since_alert >= CUSUM_ADAPT_TICKS {
                // Slowly shift μ toward the current level (deliberate baseline adaptation
                // after 60 s of sustained alert, not automatic drift like EWMA).
                self.mu = self.mu * 0.9 + score * 0.1;
                self.s_pos = CUSUM_H / 2.0; // partial reset, stay sensitive
                self.ticks_since_alert = 0;
            }
        } else {
            self.ticks_since_alert = 0;
        }
        triggered
    }
}

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
                cpu_trend: TrendDirection::Stable,
                ram_trend: TrendDirection::Stable,
                temp_trend: TrendDirection::Stable,
                cpu_delta_pct: 0.0,
                ram_delta_gb: 0.0,
                top_culprit: String::new(),
                impact_level: ImpactLevel::Healthy,
                impact_duration_s: 0,
                one_liner: "System idle.".to_string(),
                ai_agent_count: 0,
                ai_agent_ram_gb: 0.0,
                irq_per_sec: None,
                swap_used_gb: None,
                swap_total_gb: None,
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
                urgency: Urgency::Monitor,
                culprit_category: CulpritCategory::Unknown,
                claude_agents: Vec::new(),
                stranded_idle_pids: Vec::new(),
                orphan_pids: Vec::new(),
                zombie_pids: Vec::new(),
                crashed_agent_pids: Vec::new(),
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

// ── IRQ rate reader (Linux only) ──────────────────────────────────────────────

#[cfg(target_os = "linux")]
static PREV_IRQ_STATE: std::sync::Mutex<Option<(u64, std::time::Instant)>> =
    std::sync::Mutex::new(None);

/// Read total hardware interrupt rate (interrupts/sec) by diffing /proc/interrupts.
/// Returns None on first call (no delta available yet) or on parse error.
#[cfg(target_os = "linux")]
fn read_irq_per_sec() -> Option<u64> {
    let content = std::fs::read_to_string("/proc/interrupts").ok()?;
    // Sum all numeric values on each line after the first colon-delimited IRQ label.
    // Header line (first) has no colon — skip naturally via splitn.
    let total: u64 = content
        .lines()
        .skip(1) // skip the CPU0 CPU1 ... header
        .filter_map(|line| {
            let after_colon = line.splitn(2, ':').nth(1)?;
            Some(
                after_colon
                    .split_whitespace()
                    .filter_map(|s| s.parse::<u64>().ok())
                    .sum::<u64>(),
            )
        })
        .sum();

    let now = std::time::Instant::now();
    let mut prev_guard = PREV_IRQ_STATE.lock().ok()?;
    let result = match *prev_guard {
        Some((prev_total, prev_time)) => {
            let elapsed = now.duration_since(prev_time).as_secs_f64();
            if elapsed > 0.1 {
                Some(((total.saturating_sub(prev_total)) as f64 / elapsed) as u64)
            } else {
                None
            }
        }
        None => None,
    };
    *prev_guard = Some((total, now));
    result
}

/// Spawns a background Tokio task that refreshes hardware state every 2 seconds.
/// Updates the SharedState in place for the MCP server to read.
pub async fn start_collector(state: SharedState, db: persistence::DbHandle, ring: SnapshotRing) {
    let mut sys = System::new_all();
    let mut ewma = EwmaStore::default();
    let mut tick_count: u32 = 0;
    let mut cusum = CusumState::new();
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

    // Per-PID consecutive idle tick counter for non-orchestrator claude processes.
    // Only processes where is_orchestrator=false are tracked here.
    let mut agent_idle_ticks: HashMap<u32, u32> = HashMap::new();
    // D-state I/O blocking: consecutive ticks where /proc/<pid>/stat shows 'D' state.
    let mut agent_d_state_ticks: HashMap<u32, u32> = HashMap::new();

    // Orphan detection: all PIDs that were descendants of any claude process
    // last tick. Any survivor this tick with PPID=1 is an orphan.
    let mut prev_claude_descendants: std::collections::HashSet<u32> = std::collections::HashSet::new();
    // Crash detection: track claude PIDs seen last tick; disappearances = crashes.
    let mut prev_claude_agent_pids: std::collections::HashSet<u32> = std::collections::HashSet::new();

    let self_pid = std::process::id();

    // Previous-tick values for delta and trend computation
    let mut prev_cpu_pct: f64 = 0.0;
    let mut prev_ram_used_gb: f64 = 0.0;
    let mut prev_temp: Option<f64> = None;
    // Track how long current impact level has persisted
    let mut impact_level_since: std::time::Instant = std::time::Instant::now();
    let mut current_impact_for_duration = ImpactLevel::Healthy;

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

        // Compute trend directions from tick-to-tick deltas
        let cpu_trend = impact::compute_trend_direction(cpu_pct, prev_cpu_pct, 3.0);
        let ram_trend = impact::compute_trend_direction(ram_used_gb, prev_ram_used_gb, 0.1);
        let temp_trend = match (die_temp, prev_temp) {
            (Some(cur), Some(prv)) => impact::compute_trend_direction(cur, prv, 2.0),
            _ => TrendDirection::Stable,
        };
        let cpu_delta_pct = cpu_pct - prev_cpu_pct;
        let ram_delta_gb = ram_used_gb - prev_ram_used_gb;

        #[cfg(target_os = "linux")]
        let irq_per_sec = read_irq_per_sec();
        #[cfg(not(target_os = "linux"))]
        let irq_per_sec: Option<u64> = None;

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
            cpu_trend: cpu_trend.clone(),
            ram_trend: ram_trend.clone(),
            temp_trend: temp_trend.clone(),
            cpu_delta_pct,
            ram_delta_gb,
            top_culprit: String::new(), // filled after process collection
            impact_level: ImpactLevel::Healthy, // filled after impact computation
            impact_duration_s: 0, // filled after impact computation
            one_liner: String::new(), // filled after all fields are known
            ai_agent_count: 0,    // filled after process collection
            ai_agent_ram_gb: 0.0, // filled after process collection
            irq_per_sec,
            swap_used_gb: {
                let used = sys.used_swap();
                if used > 0 { Some(used as f64 / 1_073_741_824.0) } else { None }
            },
            swap_total_gb: {
                let total = sys.total_swap();
                if total > 0 { Some(total as f64 / 1_073_741_824.0) } else { None }
            },
        };
        let (headroom, headroom_reason) = impact::compute_headroom(&hw);
        hw.headroom = headroom;
        hw.headroom_reason = headroom_reason;

        // Update previous-tick values for next iteration
        prev_cpu_pct = cpu_pct;
        prev_ram_used_gb = ram_used_gb;
        prev_temp = die_temp;

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

            // Dominant-resource weighting: the bottleneck resource drives the score.
            // 70% weight on the anomaly-type primary resource, 30% on secondary.
            // In the balanced case, the higher of cpu/ram gets 65%, lower gets 35%.
            // This prevents a process using only one resource heavily from scoring
            // lower than a process using both resources moderately.
            let cpu_norm = (eff_cpu / 100.0).min(1.0);
            let ram_norm = (eff_ram / ram_total_gb.max(1.0)).min(1.0);
            let blame_score = match anomaly_type {
                AnomalyType::ThermalThrottle | AnomalyType::CpuSaturation => {
                    0.70 * cpu_norm + 0.30 * ram_norm
                }
                AnomalyType::MemoryPressure => {
                    0.30 * cpu_norm + 0.70 * ram_norm
                }
                _ => {
                    let dominant = cpu_norm.max(ram_norm);
                    let recessive = cpu_norm.min(ram_norm);
                    0.65 * dominant + 0.35 * recessive
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

        let cusum_triggered = cusum.update(score);

        let top_process_cpu_pct = process_infos
            .iter()
            .map(|p| p.cpu_pct)
            .fold(0.0_f64, f64::max);
        let impact_level = impact::score_to_level_with_context(
            score,
            cusum_triggered,
            cpu_pct,
            ram_pct,
            top_process_cpu_pct,
        );
        let culprit = process_infos.first().cloned();
        let groups = grouping::build_groups(&process_infos);

        // ── Claude sub-agent breakdown ────────────────────────────────────
        // Scan sys.processes() directly so idle sub-agents below the
        // blame_score filter threshold are still visible.
        let claude_agents: Vec<ClaudeAgentInfo> = sys
            .processes()
            .iter()
            .filter_map(|(pid, process)| {
                let name =
                    grouping::normalize_process_name(&process.name().to_string_lossy());
                if !name.to_lowercase().contains("claude") {
                    return None;
                }
                let pid_u32 = usize::from(*pid) as u32;
                // Skip SDK memory-monitor subprocess (watches another claude's statm).
                if grouping::is_memory_monitor_process(pid_u32) {
                    return None;
                }
                let meta = grouping::read_claude_cmdline(pid_u32)
                    .unwrap_or_default();
                let current_ram_gb = process.memory() as f64 / 1_073_741_824.0;
                let current_cpu_norm = process.cpu_usage() as f64 / cpu_count;
                // RAM growth rate: signed slow-EWMA delta / slow time constant (~40s).
                // Positive = context/heap growing; negative = shrinking after task.
                let ram_growth_gb_per_sec = ewma.get(pid_u32).and_then(|b| {
                    let (_, ram_delta) = b.signed_slow_delta(current_cpu_norm, current_ram_gb)?;
                    if ram_delta.abs() > 0.005 {
                        // Slow EWMA time constant: 1/alpha_slow = 1/0.05 = 20 ticks = 40s
                        Some(ram_delta / 40.0)
                    } else {
                        None
                    }
                });
                // Spin-loop detection: high per-process CPU with low system IRQ rate
                // signals a CPU-bound busy-wait rather than real I/O work.
                // Catches V8 GC runaway (#22275) and post-MCP-response CPU spin (#36729).
                // Threshold: >40% CPU on this process AND system IRQ < 5000/s AND
                // system-wide CPU confirms this process is the dominant consumer.
                let suspected_spin_loop = {
                    let this_cpu = process.cpu_usage() as f64;
                    let low_irq = hw.irq_per_sec.map_or(false, |irq| irq < 5_000);
                    if this_cpu >= 40.0 && low_irq && hw.cpu_usage_pct >= 35.0 {
                        Some(true)
                    } else {
                        None
                    }
                };
                // GC pressure: Bun/Node accumulates session render state; high RAM
                // means GC is thrashing. /clear resets the buffer (2GB → 160MB).
                // Also flag long-running sessions (>4h) with growing RAM as pre-crisis.
                let uptime_s = ewma.get(pid_u32).map(|b| b.samples as u64 * 2);
                let long_running = uptime_s.map_or(false, |s| s > 4 * 3600);
                let gc_pressure = if current_ram_gb >= 1.5 {
                    Some("critical".to_string())
                } else if current_ram_gb >= 0.8 {
                    Some("warn".to_string())
                } else if long_running && current_ram_gb >= 0.4 && ram_growth_gb_per_sec.map_or(false, |r| r > 0.0) {
                    // Long session + RAM growing → pre-warn before hitting 800MB threshold
                    Some("accumulating".to_string())
                } else {
                    None
                };
                // Fast RAM spike detection: compare current RAM against fast EWMA baseline.
                // A >300MB gap in a single tick indicates runaway allocation — SIGWINCH/resize
                // OOM pattern (1GB→21GB in 6s observed). Uses fast EWMA (α=0.4, ~5s window)
                // as baseline; needs at least WARMUP_FAST samples to avoid false positives.
                let ram_spike = ewma.get(pid_u32).and_then(|b| {
                    if b.samples >= crate::ewma::WARMUP_FAST + 1 {
                        let fast_ram = b.fast_ram();
                        if fast_ram > 0.05 && current_ram_gb - fast_ram > 0.3 {
                            Some(true)
                        } else {
                            None
                        }
                    } else {
                        None
                    }
                });
                // D-state I/O blocking + VSZ/RSS ratio detection: both read /proc/<pid>/stat.
                // D = uninterruptible sleep (blocking on disk/network/filesystem).
                // On WSL2 this causes "thinking" delays of 1-6 minutes (#22855).
                // Flag after 2 consecutive D-state ticks (~4s) to avoid transient noise.
                //
                // VSZ/RSS ratio > 50 indicates V8 heap fragmentation or mmap/munmap
                // thrashing (60Hz allocation loop, #18280). VSZ field 23 (0-indexed from
                // state: index 20), RSS field 24 (index 21) per /proc/[pid]/stat layout.
                #[cfg(target_os = "linux")]
                let (suspected_io_block, suspected_alloc_thrash) = {
                    let stat_content = std::fs::read_to_string(format!("/proc/{}/stat", pid_u32)).ok();
                    let is_d_state = stat_content
                        .as_deref()
                        .and_then(|s| s.rfind(')').map(|i| s[i + 2..].trim_start().starts_with('D')))
                        .unwrap_or(false);
                    let ticks = agent_d_state_ticks.entry(pid_u32).or_insert(0);
                    if is_d_state { *ticks += 1; } else { *ticks = 0; }
                    let io_block = if *ticks >= 2 { Some(true) } else { None };

                    // VSZ/RSS ratio: fields at index 20 (vsize bytes) and 21 (rss pages)
                    // after the closing ')' in /proc/<pid>/stat.
                    let alloc_thrash = stat_content.as_deref().and_then(|s| {
                        let after = &s[s.rfind(')')? + 2..];
                        let fields: Vec<&str> = after.split_whitespace().collect();
                        if fields.len() > 21 {
                            let vsize_bytes: u64 = fields[20].parse().ok()?;
                            let rss_pages: u64 = fields[21].parse().ok()?;
                            if rss_pages > 0 {
                                let vsz_pages = vsize_bytes / 4096;
                                let ratio = vsz_pages / rss_pages;
                                if ratio > 50 { Some(true) } else { None }
                            } else {
                                None
                            }
                        } else {
                            None
                        }
                    });
                    (io_block, alloc_thrash)
                };
                #[cfg(not(target_os = "linux"))]
                let suspected_io_block: Option<bool> = None;
                #[cfg(not(target_os = "linux"))]
                let suspected_alloc_thrash: Option<bool> = None;
                Some(ClaudeAgentInfo {
                    pid: pid_u32,
                    session_id: meta.session_id,
                    is_orchestrator: meta.is_orchestrator,
                    ram_gb: current_ram_gb,
                    cpu_pct: process.cpu_usage() as f64,
                    ram_growth_gb_per_sec,
                    suspected_spin_loop,
                    gc_pressure,
                    uptime_s,
                    ram_spike,
                    suspected_io_block,
                    suspected_alloc_thrash,
                })
            })
            .collect();

        // ── Stranded idle agent detection ────────────────────────────────
        // Update per-PID idle tick counters for non-orchestrator claude processes.
        // Only sub-agents (is_orchestrator=false) are tracked; the orchestrator
        // legitimately idles while waiting for tool responses.
        for agent in &claude_agents {
            if !agent.is_orchestrator {
                if agent.cpu_pct < crate::thresholds::AGENT_IDLE_CPU_THRESHOLD_PCT {
                    *agent_idle_ticks.entry(agent.pid).or_insert(0) += 1;
                } else {
                    agent_idle_ticks.insert(agent.pid, 0);
                }
            }
        }
        // Evict PIDs that no longer exist
        agent_idle_ticks.retain(|pid, _| active_pids.contains(pid));
        agent_d_state_ticks.retain(|pid, _| active_pids.contains(pid));

        let stranded_idle_pids: Vec<u32> = agent_idle_ticks
            .iter()
            .filter(|(_, &ticks)| ticks >= crate::thresholds::AGENT_IDLE_STRANDED_TICKS)
            .map(|(pid, _)| *pid)
            .collect();

        // ── Orphan process detection ──────────────────────────────────────
        // Build parent→children map for all live processes.
        let mut children_of: HashMap<u32, Vec<u32>> = HashMap::new();
        for (pid, process) in sys.processes() {
            let pid_u32 = usize::from(*pid) as u32;
            if let Some(parent) = process.parent() {
                children_of
                    .entry(usize::from(parent) as u32)
                    .or_default()
                    .push(pid_u32);
            }
        }
        // BFS: collect all living descendants of each claude PID.
        let claude_pids: std::collections::HashSet<u32> =
            claude_agents.iter().map(|a| a.pid).collect();
        let mut claude_descendants: std::collections::HashSet<u32> =
            std::collections::HashSet::new();
        let mut queue: Vec<u32> = claude_pids.iter().copied().collect();
        while let Some(pid) = queue.pop() {
            if let Some(kids) = children_of.get(&pid) {
                for &child in kids {
                    if claude_descendants.insert(child) {
                        queue.push(child);
                    }
                }
            }
        }
        // Orphans: previously tracked claude descendants that are still alive
        // but now have PPID=1 (reparented to init after their parent exited).
        let init_pid: u32 = 1;
        let mut orphan_pids: Vec<u32> = prev_claude_descendants
            .iter()
            .filter(|&&pid| {
                // Still alive this tick
                sys.processes()
                    .contains_key(&sysinfo::Pid::from(pid as usize))
                    // Reparented to init (orphaned)
                    && children_of
                        .get(&init_pid)
                        .map_or(false, |v| v.contains(&pid))
            })
            .copied()
            .collect();
        orphan_pids.sort_unstable();

        // Cold-start MCP orphan detection: scan for bun/node processes that are
        // children of init (PPID=1) and pegging CPU. These are likely MCP plugin
        // subprocesses from a previous Claude session that exited ungracefully.
        // Catches issue github.com/anthropics/claude-code/issues/39170.
        // Excluded: processes already tracked as active claude descendants.
        if let Some(init_children) = children_of.get(&init_pid) {
            for &pid in init_children {
                if claude_descendants.contains(&pid) || orphan_pids.contains(&pid) {
                    continue;
                }
                if let Some(proc) = sys.processes().get(&sysinfo::Pid::from(pid as usize)) {
                    let name = proc.name().to_string_lossy().to_lowercase();
                    let is_mcp_candidate = name.starts_with("bun")
                        || name.starts_with("node")
                        || name.starts_with("deno")
                        || name.starts_with("python");
                    let high_cpu = proc.cpu_usage() as f64 > 50.0;
                    if is_mcp_candidate && high_cpu {
                        orphan_pids.push(pid);
                    }
                }
            }
            orphan_pids.sort_unstable();
            orphan_pids.dedup();
        }

        prev_claude_descendants = claude_descendants;

        // ── Zombie process detection ──────────────────────────────────────
        // Detect Z-state processes that are descendants of any claude process.
        // Zombies hold a PID slot and indicate the parent is not reaping them.
        #[cfg(target_os = "linux")]
        let zombie_pids: Vec<u32> = {
            let all_claude_and_descendants: std::collections::HashSet<u32> = claude_pids
                .iter()
                .chain(prev_claude_descendants.iter())
                .copied()
                .collect();
            let mut zs: Vec<u32> = Vec::new();
            for pid in &all_claude_and_descendants {
                // A process is a zombie if any of its children are in Z state.
                // Check /proc/<child>/stat field 3 == 'Z'.
                if let Some(kids) = children_of.get(pid) {
                    for &child in kids {
                        let stat_path = format!("/proc/{}/stat", child);
                        if let Ok(stat) = std::fs::read_to_string(&stat_path) {
                            // stat format: pid (name) state ...
                            // State is the 3rd token after the closing paren of name
                            if let Some(after_paren) = stat.rfind(')') {
                                let rest = stat[after_paren + 2..].trim_start();
                                if rest.starts_with('Z') {
                                    zs.push(child);
                                }
                            }
                        }
                    }
                }
            }
            zs.sort_unstable();
            zs
        };
        #[cfg(not(target_os = "linux"))]
        let zombie_pids: Vec<u32> = Vec::new();

        // ── Claude crash detection ────────────────────────────────────────────
        // PIDs tracked last tick that no longer appear in the process list.
        // These likely crashed (Bun segfault, OOM kill) rather than exiting cleanly.
        // First tick prev_claude_agent_pids is empty, so no false positives.
        let current_claude_pid_set: std::collections::HashSet<u32> =
            claude_agents.iter().map(|a| a.pid).collect();
        let mut crashed_agent_pids: Vec<u32> = prev_claude_agent_pids
            .iter()
            .filter(|&&pid| !current_claude_pid_set.contains(&pid)
                // Exclude PIDs that are now orphans (already reported separately)
                && !orphan_pids.contains(&pid))
            .copied()
            .collect();
        crashed_agent_pids.sort_unstable();
        prev_claude_agent_pids = current_claude_pid_set;

        // Check for agent accumulation — AgentAccumulation now only fires when
        // at least one non-orchestrator claude sub-agent is genuinely idle
        // (stranded after its task completed). This prevents false positives
        // when multiple agents are all actively working in parallel.
        let (anomaly_type, culprit_group) =
            if !stranded_idle_pids.is_empty() {
                // Confirmed stranded agents: fire AgentAccumulation on the agent group
                if let Some(agent_group) = impact::detect_agent_accumulation(&groups) {
                    let non_agent_cpu_hog = groups.iter().find(|g| {
                        g.name != agent_group.name
                            && g.total_cpu_pct > agent_group.total_cpu_pct * 3.0
                    });
                    if let Some(hog) = non_agent_cpu_hog {
                        (anomaly_type, Some(hog.clone()))
                    } else {
                        (AnomalyType::AgentAccumulation, Some(agent_group.clone()))
                    }
                } else {
                    (anomaly_type, groups.first().cloned())
                }
            } else if let Some(agent_group) = impact::detect_agent_accumulation(&groups) {
                // Agents exist but none are stranded — use regular blame, not AgentAccumulation
                let non_agent_cpu_hog = groups.iter().find(|g| {
                    g.name != agent_group.name
                        && g.total_cpu_pct > agent_group.total_cpu_pct * 3.0
                });
                if let Some(hog) = non_agent_cpu_hog {
                    (anomaly_type, Some(hog.clone()))
                } else {
                    (anomaly_type, groups.first().cloned())
                }
            } else {
                (anomaly_type, groups.first().cloned())
            };

        let impact_msg = impact::impact_message(&impact_level, &anomaly_type);
        let fix = impact::suggest_fix(culprit.as_ref(), culprit_group.as_ref(), &anomaly_type);

        let urgency = impact::compute_urgency(&impact_level, &cpu_trend, &ram_trend);
        let culprit_category =
            impact::classify_culprit_from_blame(culprit_group.as_ref(), culprit.as_ref());

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
            urgency,
            culprit_category,
            claude_agents,
            stranded_idle_pids,
            orphan_pids,
            zombie_pids,
            crashed_agent_pids,
        };

        // ── Enrich hw snapshot with post-process fields ──────────────────
        // Top culprit summary
        hw.top_culprit = if let Some(g) = &blame.culprit_group {
            if blame.anomaly_score > 0.1 && g.process_count > 1 {
                format!(
                    "{} ({:.1}GB, {:.0}% CPU, {} procs)",
                    g.name, g.total_ram_gb, g.total_cpu_pct, g.process_count
                )
            } else if blame.anomaly_score > 0.1 {
                if let Some(p) = &blame.culprit {
                    format!("{} ({:.0}% CPU, {:.1}GB)", p.cmd, p.cpu_pct, p.ram_gb)
                } else {
                    String::new()
                }
            } else {
                String::new()
            }
        } else if let Some(p) = &blame.culprit {
            if blame.anomaly_score > 0.1 {
                format!("{} ({:.0}% CPU, {:.1}GB)", p.cmd, p.cpu_pct, p.ram_gb)
            } else {
                String::new()
            }
        } else {
            String::new()
        };

        hw.impact_level = impact_level.clone();

        // Populate AI agent aggregate counts from groups
        let (agent_count, agent_ram) = groups
            .iter()
            .filter(|g| impact::is_known_agent(&g.name))
            .fold((0u32, 0.0f64), |(cnt, ram), g| {
                (cnt + g.process_count as u32, ram + g.total_ram_gb)
            });
        hw.ai_agent_count = agent_count;
        hw.ai_agent_ram_gb = agent_ram;

        // Track impact duration
        if impact_level != current_impact_for_duration {
            current_impact_for_duration = impact_level.clone();
            impact_level_since = std::time::Instant::now();
        }
        hw.impact_duration_s = impact_level_since.elapsed().as_secs();

        // Build one-liner
        hw.one_liner = build_one_liner(&hw, &blame);

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
                debug!(
                    tick = tick_count,
                    "RAM pressure flapping detected, suppressing alerts"
                );
            }
            if disk_flapping {
                debug!(
                    tick = tick_count,
                    "Disk pressure flapping detected, suppressing alerts"
                );
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
            let alert_cpu_prev = if debounced_cpu {
                prev_cpu_saturated
            } else {
                cpu_saturated
            };
            let alert_throttle_prev = if debounced_throttle {
                prev_throttling
            } else {
                throttling
            };

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
        ring.push(crate::ring_buffer::RingEntry {
            hw: hw.clone(),
            anomaly_type: blame.anomaly_type.clone(),
            impact_level: blame.impact_level.clone(),
            anomaly_score: blame.anomaly_score,
        });

        // ── Persist snapshot every 15 ticks (~30s) ────────────────────────
        // The ring buffer holds ~1 hour of data at 2s resolution for recent
        // queries (session_health, hardware_trend last_1h). The DB is only
        // needed for long-term trends (last_24h+) and survives restarts.
        // Writing every 30s instead of 10s reduces disk I/O by 3x.

        if tick_count == 1 || tick_count.is_multiple_of(15) {
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
    #[cfg(target_os = "windows")]
    {
        read_battery_windows()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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

/// Windows: Query battery via WMIC (available on all Windows versions).
/// Parses `wmic path Win32_Battery get EstimatedChargeRemaining,BatteryStatus /format:csv`.
#[cfg(target_os = "windows")]
fn read_battery_windows() -> Option<BatteryStatus> {
    use std::process::Command;

    // Use PowerShell to query WMI for battery info (wmic is deprecated but still works)
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_Battery | Select-Object -Property EstimatedChargeRemaining,BatteryStatus,EstimatedRunTime | ConvertTo-Json",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return None; // No battery present (desktop PC)
    }

    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    // Handle both single object and array (multi-battery)
    let obj = if v.is_array() { v.get(0)? } else { &v };

    let percentage = obj
        .get("EstimatedChargeRemaining")
        .and_then(|v| v.as_f64())? as f32;

    // BatteryStatus: 1=Discharging, 2=AC/Charging, 3=FullyCharged, ...
    let status = obj
        .get("BatteryStatus")
        .and_then(|v| v.as_u64())
        .unwrap_or(1);
    let is_charging = status == 2 || status == 3 || status == 6 || status == 7 || status == 8 || status == 9;

    let time_to_empty = if !is_charging {
        obj.get("EstimatedRunTime")
            .and_then(|v| v.as_u64())
            .filter(|&v| v < 71_582) // WMI uses 71582 for "unknown"
            .map(|v| v as u32)
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

// ── One-Liner Builder ────────────────────────────────────────────────────────

fn build_one_liner(hw: &HwSnapshot, blame: &ProcessBlame) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(5);

    // CPU with trend
    parts.push(format!("CPU {:.0}% {}", hw.cpu_usage_pct, hw.cpu_trend));

    // RAM with trend and pressure tag if elevated
    let ram_pressure_tag = match hw.ram_pressure {
        RamPressure::Warn => " [warn]",
        RamPressure::Critical => " [critical]",
        RamPressure::Normal => "",
    };
    parts.push(format!(
        "RAM {:.1}/{:.0}GB {}{}",
        hw.ram_used_gb, hw.ram_total_gb, hw.ram_trend, ram_pressure_tag
    ));

    // Disk pressure if elevated
    match hw.disk_pressure {
        DiskPressure::Warn => parts.push(format!(
            "disk {:.0}/{:.0}GB [warn]",
            hw.disk_used_gb, hw.disk_total_gb
        )),
        DiskPressure::Critical => parts.push(format!(
            "disk {:.0}/{:.0}GB [critical]",
            hw.disk_used_gb, hw.disk_total_gb
        )),
        DiskPressure::Normal => {}
    }

    // Temp / throttle
    if let Some(t) = hw.die_temp_celsius {
        if hw.throttling {
            parts.push(format!("{:.0}C [THROTTLING]", t));
        } else if t > 70.0 {
            parts.push(format!("{:.0}C {}", t, hw.temp_trend));
        }
    } else if hw.throttling {
        parts.push("[THROTTLING]".to_string());
    }

    // Top culprit if present
    if !hw.top_culprit.is_empty() {
        parts.push(hw.top_culprit.clone());
    }

    // Urgency
    let action = match blame.urgency {
        Urgency::ActNow => " -- act now",
        Urgency::ActSoon => " -- act soon",
        Urgency::Monitor => "",
    };

    format!("{}{}", parts.join(", "), action)
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
                && p.cmd()
                    .iter()
                    .any(|arg| arg.to_string_lossy().contains("serve"))
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
        #[cfg(target_os = "windows")]
        let kill_hint = format!("taskkill /F /PID {}", pid_list.replace(' ', " /PID "));
        #[cfg(not(target_os = "windows"))]
        let kill_hint = format!("kill {}", pid_list);
        startup_warnings.push(format!(
            "{} stale axon serve instance(s) detected (PIDs: {}). Kill them: {}",
            axon_siblings.len(),
            pid_list,
            kill_hint,
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

#[cfg(target_os = "windows")]
fn detect_platform_info(sys: &System) -> (String, String) {
    let chip = sys
        .cpus()
        .first()
        .map(|cpu| cpu.brand().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Unknown CPU".to_string());

    // Try to get the machine model from WMI (e.g. "Dell XPS 15 9520", "Surface Laptop 5")
    let model_id = detect_windows_model()
        .unwrap_or_else(|| sysinfo::System::host_name().unwrap_or_else(|| "Windows PC".to_string()));

    (model_id, chip)
}

#[cfg(target_os = "windows")]
fn detect_windows_model() -> Option<String> {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_ComputerSystem | Select-Object Manufacturer,Model | ConvertTo-Json",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    let v: serde_json::Value = serde_json::from_str(text).ok()?;
    let mfr = v.get("Manufacturer").and_then(|v| v.as_str()).unwrap_or("");
    let model = v.get("Model").and_then(|v| v.as_str()).unwrap_or("");
    if model.is_empty()
        || model.contains("To Be Filled")
        || model.contains("System Product Name")
    {
        return None;
    }
    if mfr.is_empty() || mfr == "OEMGR" || mfr.contains("To Be Filled") {
        Some(model.to_string())
    } else {
        Some(format!("{} {}", mfr, model))
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
