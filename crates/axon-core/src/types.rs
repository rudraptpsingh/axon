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

// ── Core Data Types ───────────────────────────────────────────────────────────

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
}

// ── Alerts ───────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum AlertSeverity {
    Warning,
    Critical,
    Resolved,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
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
    pub cpu_avg: f64,
    pub cpu_max: f64,
    pub ram_avg: f64,
    pub ram_max: f64,
    pub temp_avg: Option<f64>,
    pub temp_max: Option<f64>,
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
