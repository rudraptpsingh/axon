use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

// ── Enums ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum RamPressure {
    Normal,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum DiskPressure {
    Normal,
    Warn,
    Critical,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AnomalyType {
    None,
    MemoryPressure,
    CpuSaturation,
    ThermalThrottle,
    GeneralSlowdown,
    AgentAccumulation,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ImpactLevel {
    Healthy,
    Degrading,
    Strained,
    Critical,
}

// ── Headroom ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomLevel {
    Adequate,
    Limited,
    Insufficient,
}

// ── Trend Direction ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TrendDirection {
    Rising,
    Falling,
    Stable,
}

impl std::fmt::Display for TrendDirection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            TrendDirection::Rising => write!(f, "rising"),
            TrendDirection::Falling => write!(f, "falling"),
            TrendDirection::Stable => write!(f, "stable"),
        }
    }
}

// ── Urgency Level ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum Urgency {
    Monitor,
    ActSoon,
    ActNow,
}

impl std::fmt::Display for Urgency {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Urgency::Monitor => write!(f, "monitor"),
            Urgency::ActSoon => write!(f, "act_soon"),
            Urgency::ActNow => write!(f, "act_now"),
        }
    }
}

// ── Culprit Category ────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum CulpritCategory {
    BuildTool,
    Browser,
    Ide,
    AiAgent,
    System,
    Unknown,
}

impl std::fmt::Display for CulpritCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CulpritCategory::BuildTool => write!(f, "build_tool"),
            CulpritCategory::Browser => write!(f, "browser"),
            CulpritCategory::Ide => write!(f, "ide"),
            CulpritCategory::AiAgent => write!(f, "ai_agent"),
            CulpritCategory::System => write!(f, "system"),
            CulpritCategory::Unknown => write!(f, "unknown"),
        }
    }
}

// ── Core Data Types ───────────────────────────────────────────────────────────

/// Metadata about a single running claude process (orchestrator or sub-agent).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClaudeAgentInfo {
    pub pid: u32,
    /// Session ID extracted from --session argument, if present.
    pub session_id: Option<String>,
    /// True when this process carries --init or --resume flags (the orchestrator).
    pub is_orchestrator: bool,
    pub ram_gb: f64,
    pub cpu_pct: f64,
    /// RAM growth rate over the last ~40 seconds (GB/sec, positive = growing).
    /// None until the slow EWMA baseline has accumulated enough history (~16s).
    /// Use to anticipate context exhaustion before OOM: >0.01 GB/sec is notable.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_growth_gb_per_sec: Option<f64>,
    /// True when this claude PID has high CPU but system IRQ rate is low, indicating
    /// a spin loop rather than real work (V8 GC runaway, infinite loop after MCP response).
    /// See: github.com/anthropics/claude-code/issues/22275, #36729.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspected_spin_loop: Option<bool>,
    /// GC pressure level based on process RAM: "warn" (>800MB) or "critical" (>1.5GB).
    /// The Bun/Node runtime accumulates render buffer state over a session; when RAM
    /// exceeds ~1.5-2GB the GC enters a CPU-thrashing loop. Run /clear to reset.
    /// See: github.com/anthropics/claude-code/issues/22509, #30807.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gc_pressure: Option<String>,
    /// Approximate session uptime in seconds (estimated from EWMA sample count × 2s tick).
    /// None until the EWMA tracker has enough samples. Useful for correlating long sessions
    /// with GC pressure — CPU thrashing typically appears after 6-8h of uptime.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uptime_s: Option<u64>,
    /// True when this claude PID's RAM jumped >300MB above its fast EWMA baseline in a single
    /// tick (~2s). Indicates runaway memory allocation — likely triggered by rapid terminal
    /// resize (SIGWINCH burst) on a large session. OOM kill risk within seconds.
    /// See: github.com/anthropics/claude-code/issues/39022.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ram_spike: Option<bool>,
    /// True when this claude process is in D-state (uninterruptible I/O wait) for 2+
    /// consecutive ticks (~4s). Indicates filesystem/network blocking — common on WSL2
    /// where filesystem bridging is slow, causing 1-6 minute "thinking" delays.
    /// See: github.com/anthropics/claude-code/issues/22855.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspected_io_block: Option<bool>,
    /// True on Linux when VSZ/RSS page ratio > 50 for this claude process.
    /// Indicates V8 heap fragmentation or mmap/munmap thrashing (60Hz allocation loop
    /// introduced in v2.1.7 "terminal rendering optimization"). Manifests as 50-80% CPU
    /// while apparently idle, abnormally large virtual address space (73-85 GB VSZ with
    /// only ~600 MB RSS). Distinct from spin-loop: IRQ rate may be moderate.
    /// See: github.com/anthropics/claude-code/issues/18280.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub suspected_alloc_thrash: Option<bool>,
    /// True on Linux when the FD table size (FDSize from /proc/<pid>/status) exceeds
    /// 4096. Indicates an fs.watch / inotify watcher leak: Node.js file watchers
    /// accumulate without being closed. Once the process hits ulimit -n (typically
    /// 1024–65536), every subsequent open() fails with EMFILE. Observed at 757,812
    /// open descriptors before crash.
    /// See: github.com/anthropics/claude-code/issues/11136.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fd_leak: Option<bool>,
    /// Rate at which this claude PID spawns+reaps child processes (children/sec over
    /// last 2s). Values > 20/s indicate a zombie storm or tight subprocess loop —
    /// the statusLine render bug (#34092) produced 185 zombies/sec, RSS growing
    /// 400MB → 17GB. None when no children are spawning.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub child_churn_rate_per_sec: Option<f64>,
    /// Disk I/O read throughput for this PID in MB/sec (Linux, from /proc/<pid>/io).
    /// High read bytes with low CPU% indicates a polling/re-read loop — the cowork-svc
    /// pattern reads a 213MB binary every 2s (195MB/s). None when below threshold.
    /// See: github.com/anthropics/claude-code/issues/22543.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub io_read_mb_per_sec: Option<f64>,
    /// Seconds this PID has been above 30% CPU with no new child spawns and minimal
    /// I/O. Indicates a userspace spin/poll loop (pread64/futex pattern) rather than
    /// real work. Distinct from suspected_spin_loop (which fires on high IRQ-free CPU
    /// at system level); this fires on sustained low-productivity CPU use.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub idle_cpu_spin_secs: Option<u64>,
    /// EWMA-smoothed RSS growth rate in MB/hr. Fires before the gc_pressure threshold
    /// (800MB). Values > 50 MB/hr are notable; > 300 MB/hr is a crash trajectory.
    /// Catches the node-pty ArrayBuffer leak cluster (10+ issues, 72GB/hr observed).
    /// See: github.com/anthropics/claude-code/issues/31511, #33118.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rss_growth_rate_mb_per_hr: Option<f64>,
    /// Size (MB) of the largest ~/.claude/projects/**/*.jsonl session file for this
    /// process's session_id. Files > 40MB cause synchronous full-load hangs —
    /// observed as infinite "thinking" spin with no CPU activity (#21022).
    /// None when no large session files are found or session_id is unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub large_session_file_mb: Option<f64>,
    /// True when this bun/node process has uptime > 4h AND rss_growth_rate_mb_per_hr
    /// > 300. Predicts a mimalloc OOM crash within the next 1-2h — matches the
    /// >      bun memory leak trajectory (#21875, #29192, #33118). Act before the crash.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bun_crash_trajectory: Option<bool>,
    /// Instantaneous count of zombie children for this claude PID (complement to
    /// child_churn_rate_per_sec for burst detection). Non-zero means the process is
    /// not reaping children fast enough — zombie accumulation stalls new spawns once
    /// the PID table fills. See: github.com/anthropics/claude-code/issues/34092.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub zombie_child_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HwSnapshot {
    pub die_temp_celsius: Option<f64>,
    pub throttling: bool,
    pub ram_used_gb: f64,
    pub ram_total_gb: f64,
    pub ram_pressure: RamPressure,
    pub cpu_usage_pct: f64,
    pub disk_used_gb: f64,
    pub disk_total_gb: f64,
    pub disk_pressure: DiskPressure,
    pub headroom: HeadroomLevel,
    pub headroom_reason: String,
    pub ts: DateTime<Utc>,
    // ── Agent-context enrichment fields ──────────────────────────────────
    /// CPU trajectory: rising, falling, or stable (based on EWMA fast vs slow).
    pub cpu_trend: TrendDirection,
    /// RAM trajectory: rising, falling, or stable.
    pub ram_trend: TrendDirection,
    /// Temperature trajectory: rising, falling, or stable.
    pub temp_trend: TrendDirection,
    /// Change in CPU % since the previous collector tick.
    pub cpu_delta_pct: f64,
    /// Change in RAM (GB) since the previous collector tick.
    pub ram_delta_gb: f64,
    /// Top resource consumer (one-line summary). Empty when system is idle.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub top_culprit: String,
    /// Current impact level (mirrors process_blame.impact_level).
    pub impact_level: ImpactLevel,
    /// How long the current impact level has persisted (seconds).
    pub impact_duration_s: u64,
    /// Ultra-compact one-line summary for token-constrained agents.
    pub one_liner: String,
    /// Total number of AI agent processes (claude, cursor, windsurf) visible right now.
    /// Useful for a pre-spawn check before starting a heavy sub-agent task.
    #[serde(default)]
    pub ai_agent_count: u32,
    /// Combined RAM used by all AI agent processes (GB).
    #[serde(default)]
    pub ai_agent_ram_gb: f64,
    /// Per-second total hardware interrupt rate (Linux only). None on other platforms.
    /// High IRQ rate with moderate CPU% → real I/O work (disk/net).
    /// Near-zero IRQ with high CPU% → spin-loop or pure compute (e.g. yes, tight loop).
    /// Useful for distinguishing cargo build (high IRQ) from runaway agent (low IRQ).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub irq_per_sec: Option<u64>,
    /// Swap space currently in use (GB). High swap indicates RAM exhaustion forcing paging.
    /// Relevant for Cowork VM bundle memory pressure (#22543) and general OOM risk.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_used_gb: Option<f64>,
    /// Total swap space available (GB). None when no swap is configured.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub swap_total_gb: Option<f64>,
    /// Rate at which disk usage is growing (GB/sec). None on first tick or if stable.
    /// Positive values indicate active disk fill — can signal runaway debug-log loops
    /// (200 GB+ in ~hours, #16093) or task .output file accumulation (537 GB, #26911).
    /// Fires when delta between ticks exceeds 50 MB (0.05 GB) over 2 seconds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub disk_fill_rate_gb_per_sec: Option<f64>,
    /// System-wide open file descriptor pool utilization (%). Linux only, from
    /// /proc/sys/fs/file-nr: (allocated - free) / max * 100. Values > 90% mean the
    /// system is close to the global FD hard limit — any process attempting open()
    /// will get ENFILE. Affects all processes, not just the target. None on non-Linux.
    /// See: github.com/anthropics/claude-code/issues/11136.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system_fd_pct: Option<f64>,
    /// True when MemFree + SwapFree < 64MB and SwapFree == 0 (Linux /proc/meminfo).
    /// This is the Linux hard-freeze precondition: with no swap and no free RAM, the
    /// next large allocation triggers OOM freeze — the kernel stalls indefinitely
    /// rather than killing processes. Recovery requires reboot. None on non-Linux.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub oom_freeze_risk: Option<bool>,
    /// Total size of ~/.claude/ directory in GB, sampled every 30 ticks (~60s).
    /// Values > 1 GB warrant investigation; > 10 GB is likely a runaway debug-log
    /// or node_modules/.cache leak. Cascade failure observed at 0 bytes free from
    /// 472GB debug logs and 20GB/week .node module accumulation (#16093, #26911).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dot_claude_size_gb: Option<f64>,
    /// Count of currently running MCP server processes (python/node/bun/deno with
    /// MCP-related command lines). > 4 simultaneous MCP servers accumulate per-session
    /// commit charge; > 8 risks address space exhaustion on 32-bit hosts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mcp_server_count: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessInfo {
    pub pid: u32,
    pub cmd: String,
    pub cpu_pct: f64,
    pub ram_gb: f64,
    pub blame_score: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessGroup {
    pub name: String,
    pub process_count: usize,
    pub total_cpu_pct: f64,
    pub total_ram_gb: f64,
    pub blame_score: f64,
    pub top_pid: u32,
    pub pids: Vec<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessBlame {
    pub anomaly_type: AnomalyType,
    pub impact_level: ImpactLevel,
    pub culprit: Option<ProcessInfo>,
    pub culprit_group: Option<ProcessGroup>,
    pub anomaly_score: f64,
    pub impact: String,
    pub fix: String,
    pub ts: DateTime<Utc>,
    /// PIDs of other `axon serve` instances (not self). Empty when no siblings.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stale_axon_pids: Vec<u32>,
    /// How urgent is the situation: monitor, act_soon, or act_now.
    pub urgency: Urgency,
    /// What kind of process is the culprit: build_tool, browser, ide, ai_agent, system, unknown.
    pub culprit_category: CulpritCategory,
    /// Per-process breakdown of running claude instances (orchestrator + sub-agents).
    /// Empty when no claude processes are visible.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub claude_agents: Vec<ClaudeAgentInfo>,
    /// PIDs of non-orchestrator claude processes that have been CPU-idle for
    /// more than AGENT_IDLE_STRANDED_TICKS consecutive ticks. These are
    /// the processes that trigger AgentAccumulation anomaly type.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stranded_idle_pids: Vec<u32>,
    /// PIDs of processes that were descendants of a claude process but whose
    /// parent has since exited, leaving them reparented to init (PPID=1).
    /// These are orphaned tool invocations consuming CPU/RAM with no owner.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub orphan_pids: Vec<u32>,
    /// PIDs of zombie (Z-state) processes that are descendants of any claude
    /// process. Zombies indicate the parent is not calling wait() — they hold
    /// a PID slot and accumulate until the parent exits or reaps them.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub zombie_pids: Vec<u32>,
    /// PIDs of claude processes that were tracked last tick but have now disappeared
    /// without a graceful exit — likely crashed (Bun segfault, OOM kill, SIGKILL).
    /// See: github.com/anthropics/claude-code/issues/21875 (Bun segfaults).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub crashed_agent_pids: Vec<u32>,
    /// Count of claude/bun processes with age > 86400s and RSS > 200MB. These are
    /// stale sessions that were never cleaned up — invisible wait-state accumulation
    /// that slowly drains RAM. Normal operation: 0. > 2 indicates a cleanup problem.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stale_session_count: Option<u32>,
    /// Total count of PPID=1 (reparented-to-init) claude/bun processes regardless of
    /// CPU usage. Broadens orphan_pids (which only fires for high-CPU orphans) to
    /// catch idle orphaned MCP server subprocesses consuming RAM silently.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub subagent_orphan_count_total: Option<u32>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BatteryStatus {
    pub percentage: f32,
    pub is_charging: bool,
    pub time_to_empty_min: Option<u32>,
    pub narrative: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemProfile {
    pub model_id: String,
    pub chip: String,
    pub core_count: usize,
    pub ram_total_gb: f64,
    pub os_version: String,
    pub axon_version: String,
    /// Warnings detected at startup (e.g. stale axon instances).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub startup_warnings: Vec<String>,
}

// ── Alerts ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Warning,
    Critical,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum AlertType {
    MemoryPressure,
    ThermalThrottle,
    ImpactEscalation,
    DiskPressure,
    CpuSaturation,
}

impl std::fmt::Display for AlertType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertType::MemoryPressure => write!(f, "memory_pressure"),
            AlertType::ThermalThrottle => write!(f, "thermal_throttle"),
            AlertType::ImpactEscalation => write!(f, "impact_escalation"),
            AlertType::DiskPressure => write!(f, "disk_pressure"),
            AlertType::CpuSaturation => write!(f, "cpu_saturation"),
        }
    }
}

impl std::fmt::Display for AlertSeverity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AlertSeverity::Warning => write!(f, "warning"),
            AlertSeverity::Critical => write!(f, "critical"),
            AlertSeverity::Resolved => write!(f, "resolved"),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertMetadata {
    pub ram_pct: Option<f64>,
    pub cpu_pct: Option<f64>,
    pub temp_c: Option<f64>,
    pub disk_pct: Option<f64>,
    pub culprit: Option<ProcessInfo>,
    pub culprit_group: Option<ProcessGroup>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Alert {
    pub severity: AlertSeverity,
    pub alert_type: AlertType,
    pub message: String,
    pub ts: DateTime<Utc>,
    pub metadata: AlertMetadata,
}

// ── Trend Types ──────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendBucket {
    pub bucket_start: DateTime<Utc>,
    pub sample_count: u32,
    pub avg_cpu_pct: f64,
    pub peak_cpu_pct: f64,
    pub avg_ram_gb: f64,
    pub peak_ram_gb: f64,
    pub avg_temp_celsius: Option<f64>,
    pub peak_temp_celsius: Option<f64>,
    pub anomaly_count: u32,
    pub throttle_count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendData {
    pub buckets: Vec<TrendBucket>,
    pub trend_direction: String,
    pub total_snapshots: u32,
}

// ── Session Health ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHealth {
    pub since: DateTime<Utc>,
    pub snapshot_count: u32,
    pub alert_count: u32,
    pub worst_impact_level: ImpactLevel,
    pub worst_anomaly_type: AnomalyType,
    pub avg_anomaly_score: f64,
    pub avg_cpu_pct: f64,
    pub avg_ram_gb: f64,
    pub peak_cpu_pct: f64,
    pub peak_ram_gb: f64,
    pub peak_temp_celsius: Option<f64>,
    pub throttle_event_count: u32,
    /// Number of snapshots where anomaly_type was agent_accumulation (stranded idle sub-agents).
    #[serde(default)]
    pub agent_accumulation_events: u32,
    /// Peak number of AI agent processes seen at any single snapshot during the session.
    #[serde(default)]
    pub peak_ai_agent_count: u32,
}

// ── GPU Types ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GpuSnapshot {
    /// Overall GPU busy percentage (0–100), from IOAccelerator PerformanceStatistics.
    pub utilization_pct: Option<f64>,
    /// Geometry/tiling stage utilization percentage.
    pub tiler_utilization_pct: Option<f64>,
    /// Fragment/render stage utilization percentage.
    pub renderer_utilization_pct: Option<f64>,
    /// GPU-accessible memory currently in use (bytes mapped to VRAM equivalent).
    pub vram_used_bytes: Option<u64>,
    /// Total GPU-allocated memory (bytes).
    pub vram_alloc_bytes: Option<u64>,
    /// Cumulative GPU hang/reset count since boot. Any delta signals a driver crash.
    pub recovery_count: Option<u64>,
    /// GPU model name (e.g. "Apple M2").
    pub model: Option<String>,
    /// Number of GPU cores.
    pub core_count: Option<u32>,
    /// True if a GPU was detected on this machine.  False means no GPU was
    /// found (no nvidia-smi, no DRM sysfs card, ioreg returned nothing) and
    /// all metric fields will be None.
    pub detected: bool,
    pub ts: DateTime<Utc>,
}

// ── MCP Response Envelope ─────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize)]
pub struct McpResponse<T: Serialize + Clone> {
    pub ok: bool,
    pub ts: DateTime<Utc>,
    pub data: T,
    pub narrative: String,
}

impl<T: Serialize + Clone> McpResponse<T> {
    pub fn success(data: T, narrative: String) -> Self {
        Self {
            ok: true,
            ts: Utc::now(),
            data,
            narrative,
        }
    }
}
