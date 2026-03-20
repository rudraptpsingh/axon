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
pub enum AnomalyType {
    None,
    MemoryPressure,
    CpuSaturation,
    ThermalThrottle,
    GeneralSlowdown,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum ImpactLevel {
    Healthy,
    Degrading,
    Strained,
    Critical,
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
pub struct ProcessBlame {
    pub anomaly_type: AnomalyType,
    pub impact_level: ImpactLevel,
    pub culprit: Option<ProcessInfo>,
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
