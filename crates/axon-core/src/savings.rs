//! Token & cost savings ledger.
//!
//! axon does not merely observe hardware -- every actionable signal it surfaces to an
//! AI coding agent lets that agent *avoid* a concrete, wasteful token spend: a build that
//! would OOM and be re-run, a session that would crash and have its context rebuilt from
//! scratch, a polling loop re-reading a large file into context every turn, a bloated
//! session file re-processed on every request. This module turns those prevented failures
//! into a running estimate of tokens (and therefore dollars) saved, so a user running
//! ordinary Claude sessions can look back and see: "axon saved me ~X tokens (~$Y) today."
//!
//! Two kinds of events land in the ledger:
//!   * `Detected`     -- axon caught an actionable condition and surfaced the fix. Recorded
//!                       automatically by the collector, edge-triggered so it never spams.
//!   * `AgentAction`  -- an agent (or the axon skill) explicitly confirmed it acted on a
//!                       recommendation, via the `record_savings` tool / `axon savings record`.
//!
//! Token estimates are deliberately conservative and centralised here (see `catalog`)
//! so they are easy to audit and tune. They are estimates, not measurements, and every
//! surfaced number is labelled as such.

use chrono::{DateTime, Utc};

use crate::types::*;

/// Environment override for the blended token price used to convert tokens -> USD.
/// Dollars per **million** tokens. Defaults to a conservative blended rate.
pub const TOKEN_PRICE_ENV: &str = "AXON_TOKEN_PRICE_PER_MTOK";

/// Conservative default blended price per 1M tokens (USD). Chosen to under- rather than
/// over-state savings. Override with `AXON_TOKEN_PRICE_PER_MTOK`.
pub const DEFAULT_TOKEN_PRICE_PER_MTOK: f64 = 3.0;

/// Minimum spacing between two auto-`Detected` events of the *same* category, in collector
/// ticks (2s each). A persistent condition is credited once per window, not every tick.
/// 300 ticks ~= 10 minutes.
pub const SAVINGS_DETECT_COOLDOWN_TICKS: u32 = 300;

/// Resolve the active token price (USD per 1M tokens).
pub fn price_per_mtok() -> f64 {
    std::env::var(TOKEN_PRICE_ENV)
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .filter(|v| *v > 0.0)
        .unwrap_or(DEFAULT_TOKEN_PRICE_PER_MTOK)
}

/// Convert a token count into an estimated dollar cost using the active blended price.
pub fn tokens_to_usd(tokens: u64) -> f64 {
    tokens as f64 / 1_000_000.0 * price_per_mtok()
}

// ── Categories ────────────────────────────────────────────────────────────────

/// The family of waste a surfaced signal prevents. Each maps to a conservative token
/// estimate in the catalog below.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsCategory {
    /// Heavy task (build/test) deferred while the machine had no headroom -- avoids an
    /// OOM/thrash failure plus the agent reading the error log and retrying.
    DeferredHeavyTask,
    /// A memory-critical / OOM-freeze condition caught before the session was killed --
    /// avoids full context reconstruction on a fresh session.
    PreventedOomCrash,
    /// Runaway session RAM / GC thrash caught; `/clear` recommended before the agent
    /// re-sends a bloated context on every turn.
    ContextReset,
    /// Oversized session file caught; `/compact` recommended before it is re-processed
    /// on every request (or triggers a synchronous load hang).
    ContextCompaction,
    /// A disk polling / re-read loop caught -- avoids repeatedly pulling a large file
    /// into the agent's context.
    StoppedPollingLoop,
    /// A runaway / spin-looping / crash-trajectory agent process caught before it burned
    /// a degraded, low-productivity session.
    KilledRunawayProcess,
    /// Work deferred while the CPU was thermally throttled -- avoids slow, throttled
    /// token generation.
    ThermalDefer,
    /// Accumulated stale/orphaned agent processes surfaced for cleanup before they caused
    /// redundant re-spawns and confusion.
    AgentCleanup,
    /// Runaway disk fill / disk-pressure condition caught before a crash-and-restart cycle.
    DiskCleanup,
}

impl SavingsCategory {
    pub fn as_str(&self) -> &'static str {
        match self {
            SavingsCategory::DeferredHeavyTask => "deferred_heavy_task",
            SavingsCategory::PreventedOomCrash => "prevented_oom_crash",
            SavingsCategory::ContextReset => "context_reset",
            SavingsCategory::ContextCompaction => "context_compaction",
            SavingsCategory::StoppedPollingLoop => "stopped_polling_loop",
            SavingsCategory::KilledRunawayProcess => "killed_runaway_process",
            SavingsCategory::ThermalDefer => "thermal_defer",
            SavingsCategory::AgentCleanup => "agent_cleanup",
            SavingsCategory::DiskCleanup => "disk_cleanup",
        }
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Option<Self> {
        let v = match s.trim().to_ascii_lowercase().as_str() {
            "deferred_heavy_task" | "defer" | "deferred" => SavingsCategory::DeferredHeavyTask,
            "prevented_oom_crash" | "oom" | "crash" => SavingsCategory::PreventedOomCrash,
            "context_reset" | "clear" => SavingsCategory::ContextReset,
            "context_compaction" | "compact" => SavingsCategory::ContextCompaction,
            "stopped_polling_loop" | "polling" | "reread" => SavingsCategory::StoppedPollingLoop,
            "killed_runaway_process" | "runaway" | "spin" => SavingsCategory::KilledRunawayProcess,
            "thermal_defer" | "thermal" => SavingsCategory::ThermalDefer,
            "agent_cleanup" | "cleanup" | "orphans" => SavingsCategory::AgentCleanup,
            "disk_cleanup" | "disk" => SavingsCategory::DiskCleanup,
            _ => return None,
        };
        Some(v)
    }

    pub fn all() -> &'static [SavingsCategory] {
        &[
            SavingsCategory::DeferredHeavyTask,
            SavingsCategory::PreventedOomCrash,
            SavingsCategory::ContextReset,
            SavingsCategory::ContextCompaction,
            SavingsCategory::StoppedPollingLoop,
            SavingsCategory::KilledRunawayProcess,
            SavingsCategory::ThermalDefer,
            SavingsCategory::AgentCleanup,
            SavingsCategory::DiskCleanup,
        ]
    }
}

impl std::fmt::Display for SavingsCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

/// The origin of a ledger entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavingsSource {
    /// axon detected the condition and surfaced the fix (recorded by the collector).
    Detected,
    /// An agent explicitly confirmed it acted on a recommendation.
    AgentAction,
}

impl SavingsSource {
    pub fn as_str(&self) -> &'static str {
        match self {
            SavingsSource::Detected => "detected",
            SavingsSource::AgentAction => "agent_action",
        }
    }
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        match s.trim().to_ascii_lowercase().as_str() {
            "agent_action" | "agent" | "action" | "confirmed" => SavingsSource::AgentAction,
            _ => SavingsSource::Detected,
        }
    }
}

impl std::fmt::Display for SavingsSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.as_str())
    }
}

// ── Catalog ───────────────────────────────────────────────────────────────────

/// Static, auditable description of what a category prevents and the conservative token
/// estimate we credit for it.
#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub category: SavingsCategory,
    /// Conservative estimate of tokens a single prevention saves.
    pub base_tokens: u64,
    /// Short human title for the event ("Deferred heavy build under memory pressure").
    pub title: &'static str,
    /// Default recommended/taken action ("deferred the build until RAM recovered").
    pub action: &'static str,
    /// Related upstream issue reference, when the failure mode traces to a known bug.
    pub issue_ref: Option<&'static str>,
    /// One-line rationale for the token estimate (why this number).
    pub rationale: &'static str,
}

/// Look up the catalog entry for a category. Estimates are intentionally conservative;
/// tune them here in one place.
pub fn catalog(category: SavingsCategory) -> CatalogEntry {
    use SavingsCategory::*;
    match category {
        DeferredHeavyTask => CatalogEntry {
            category,
            base_tokens: 12_000,
            title: "Deferred a heavy task under resource pressure",
            action: "deferred the build/test until the machine had headroom",
            issue_ref: None,
            rationale:
                "avoids one failed build cycle: the agent reading the OOM/error output and retrying",
        },
        PreventedOomCrash => CatalogEntry {
            category,
            base_tokens: 45_000,
            title: "Caught an OOM / hard-freeze condition before the session was killed",
            action: "freed memory / paused work before the OOM kill",
            issue_ref: Some("#39022"),
            rationale: "a killed session must rebuild its whole working context from scratch",
        },
        ContextReset => CatalogEntry {
            category,
            base_tokens: 30_000,
            title: "Caught runaway session RAM / GC thrash",
            action: "ran /clear to reset the session before GC thrash",
            issue_ref: Some("#33874"),
            rationale: "avoids re-sending a bloated context on every subsequent turn",
        },
        ContextCompaction => CatalogEntry {
            category,
            base_tokens: 18_000,
            title: "Caught an oversized session before a load hang",
            action: "ran /compact to shrink the session file",
            issue_ref: Some("#21022"),
            rationale: "avoids re-processing an oversized session on every request",
        },
        StoppedPollingLoop => CatalogEntry {
            category,
            base_tokens: 8_000,
            title: "Caught a disk polling / re-read loop",
            action: "stopped the process re-reading a large file",
            issue_ref: Some("#22543"),
            rationale: "avoids repeatedly pulling the same large file into context",
        },
        KilledRunawayProcess => CatalogEntry {
            category,
            base_tokens: 6_000,
            title: "Caught a runaway / crash-trajectory agent process",
            action: "restarted the runaway process before it degraded the session",
            issue_ref: Some("#21875"),
            rationale: "avoids a degraded, low-productivity session doing redundant work",
        },
        ThermalDefer => CatalogEntry {
            category,
            base_tokens: 4_000,
            title: "Deferred work while the CPU was thermally throttled",
            action: "paused heavy work until the CPU cooled",
            issue_ref: None,
            rationale: "avoids slow token generation during a throttle window",
        },
        AgentCleanup => CatalogEntry {
            category,
            base_tokens: 5_000,
            title: "Surfaced stale / orphaned agent processes for cleanup",
            action: "cleaned up stale/orphaned agent processes",
            issue_ref: Some("#39170"),
            rationale: "avoids redundant re-spawns and confused work across leaked sessions",
        },
        DiskCleanup => CatalogEntry {
            category,
            base_tokens: 7_000,
            title: "Caught a runaway disk-fill / disk-pressure condition",
            action: "cleared runaway files before the disk filled",
            issue_ref: Some("#26911"),
            rationale: "avoids a disk-full crash and the restart cycle that follows",
        },
    }
}

// ── Event construction ─────────────────────────────────────────────────────────

/// A single ledger entry: what axon prevented, and the estimated tokens/dollars saved.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavingsEvent {
    pub ts: DateTime<Utc>,
    pub category: SavingsCategory,
    pub source: SavingsSource,
    /// The concrete signal that triggered this event (e.g. "memory_pressure", "gc_pressure_critical").
    pub signal: String,
    /// Upstream issue reference, when applicable (e.g. "#33874").
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub issue_ref: Option<String>,
    /// Short human title.
    pub title: String,
    /// What axon observed / why this fired.
    pub detail: String,
    /// Recommended or taken action.
    pub action: String,
    pub tokens_saved: u64,
    pub usd_saved: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

impl SavingsEvent {
    /// Build an event from a category + source, filling title/action/issue/tokens from the
    /// catalog. `detail` describes the concrete observation; `signal` names the trigger.
    /// `tokens_override` lets an agent supply a measured figure instead of the estimate.
    pub fn new(
        category: SavingsCategory,
        source: SavingsSource,
        signal: impl Into<String>,
        detail: impl Into<String>,
        tokens_override: Option<u64>,
    ) -> Self {
        let entry = catalog(category);
        let tokens = tokens_override.unwrap_or(entry.base_tokens);
        SavingsEvent {
            ts: Utc::now(),
            category,
            source,
            signal: signal.into(),
            issue_ref: entry.issue_ref.map(|s| s.to_string()),
            title: entry.title.to_string(),
            detail: detail.into(),
            action: entry.action.to_string(),
            tokens_saved: tokens,
            usd_saved: tokens_to_usd(tokens),
            session_id: None,
        }
    }

    pub fn with_action(mut self, action: impl Into<String>) -> Self {
        self.action = action.into();
        self
    }

    pub fn with_session(mut self, session_id: Option<String>) -> Self {
        self.session_id = session_id;
        self
    }
}

// ── Derivation from live state ──────────────────────────────────────────────────

/// Map an edge-triggered alert to a `Detected` savings event. Returns None for alerts
/// that do not correspond to a prevented token spend.
pub fn from_alert(alert: &Alert) -> Option<SavingsEvent> {
    let (category, signal) = match (&alert.alert_type, &alert.severity) {
        (AlertType::MemoryPressure, AlertSeverity::Critical) => (
            SavingsCategory::PreventedOomCrash,
            "memory_pressure_critical",
        ),
        (AlertType::MemoryPressure, _) => {
            (SavingsCategory::DeferredHeavyTask, "memory_pressure_warn")
        }
        (AlertType::ThermalThrottle, _) => (SavingsCategory::ThermalDefer, "thermal_throttle"),
        (AlertType::ImpactEscalation, _) => {
            (SavingsCategory::DeferredHeavyTask, "impact_escalation")
        }
        (AlertType::DiskPressure, _) => (SavingsCategory::DiskCleanup, "disk_pressure"),
        (AlertType::CpuSaturation, _) => (SavingsCategory::KilledRunawayProcess, "cpu_saturation"),
        // Resolved alerts are not preventions.
        _ => return None,
    };
    if alert.severity == AlertSeverity::Resolved {
        return None;
    }
    Some(SavingsEvent::new(
        category,
        SavingsSource::Detected,
        signal,
        alert.message.clone(),
        None,
    ))
}

/// Scan the current process-blame for actionable agent conditions, returning one candidate
/// `Detected` event per distinct category present. The collector applies a per-category
/// cooldown so a persistent condition is credited at most once per window.
pub fn from_blame(blame: &ProcessBlame) -> Vec<SavingsEvent> {
    // Collect (category, signal, detail, session) candidates, then keep the first per
    // distinct category so a persistent condition yields at most one event per call.
    let mut candidates: Vec<(SavingsCategory, &'static str, String, Option<String>)> = Vec::new();

    for a in &blame.claude_agents {
        if a.gc_pressure.as_deref() == Some("critical") {
            candidates.push((
                SavingsCategory::ContextReset,
                "gc_pressure_critical",
                format!(
                    "PID {} at {:.1}GB RAM -- GC thrash imminent",
                    a.pid, a.ram_gb
                ),
                a.session_id.clone(),
            ));
        }
        if a.large_session_file_mb.is_some() || a.ctx_window_risk.as_deref() == Some("critical") {
            let mb = a.large_session_file_mb.unwrap_or(0.0);
            candidates.push((
                SavingsCategory::ContextCompaction,
                "session_file_oversized",
                format!(
                    "PID {} session file ~{:.0}MB -- load-hang / re-process risk",
                    a.pid, mb
                ),
                a.session_id.clone(),
            ));
        }
        if let Some(rate) = a.io_read_mb_per_sec {
            candidates.push((
                SavingsCategory::StoppedPollingLoop,
                "io_read_polling",
                format!(
                    "PID {} reading {:.0}MB/s with low CPU -- re-read loop",
                    a.pid, rate
                ),
                a.session_id.clone(),
            ));
        }
        if a.bun_crash_trajectory == Some(true)
            || a.suspected_spin_loop == Some(true)
            || a.rss_growth_rate_mb_per_hr.is_some_and(|r| r > 300.0)
        {
            candidates.push((
                SavingsCategory::KilledRunawayProcess,
                "runaway_agent",
                format!("PID {} on a crash/spin trajectory", a.pid),
                a.session_id.clone(),
            ));
        }
    }

    let orphan_total = blame.subagent_orphan_count_total.unwrap_or(0) as usize;
    if blame.anomaly_type == AnomalyType::AgentAccumulation
        || !blame.orphan_pids.is_empty()
        || !blame.zombie_pids.is_empty()
        || orphan_total > blame.orphan_pids.len()
        || blame.stale_session_count.unwrap_or(0) > 0
    {
        let n = orphan_total
            .max(blame.orphan_pids.len())
            .max(blame.zombie_pids.len())
            .max(blame.stale_session_count.unwrap_or(0) as usize);
        candidates.push((
            SavingsCategory::AgentCleanup,
            "agent_accumulation",
            format!("{} stale/orphaned agent process(es) detected", n),
            None,
        ));
    }

    let mut have: std::collections::HashSet<SavingsCategory> = std::collections::HashSet::new();
    let mut out: Vec<SavingsEvent> = Vec::new();
    for (cat, signal, detail, sid) in candidates {
        if have.insert(cat) {
            out.push(
                SavingsEvent::new(cat, SavingsSource::Detected, signal, detail, None)
                    .with_session(sid),
            );
        }
    }
    out
}

// ── Aggregation types ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavingsCategoryTotal {
    pub category: SavingsCategory,
    pub event_count: u32,
    pub tokens_saved: u64,
    pub usd_saved: f64,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavingsRollupBucket {
    pub bucket_start: DateTime<Utc>,
    /// Human label for the bucket (e.g. "2026-07-27" for a day).
    pub label: String,
    pub event_count: u32,
    pub tokens_saved: u64,
    pub usd_saved: f64,
}

/// Full savings report: totals, per-category breakdown, a time rollup (daily/weekly/monthly),
/// and the most recent referenced events.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SavingsReport {
    pub range_label: String,
    pub since: DateTime<Utc>,
    pub total_events: u32,
    pub total_tokens_saved: u64,
    pub total_usd_saved: f64,
    /// Events axon detected and surfaced automatically.
    pub detected_events: u32,
    /// Events an agent explicitly confirmed it acted on.
    pub confirmed_events: u32,
    pub price_per_mtok_usd: f64,
    pub by_category: Vec<SavingsCategoryTotal>,
    pub buckets: Vec<SavingsRollupBucket>,
    pub recent_events: Vec<SavingsEvent>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn category_roundtrip() {
        for c in SavingsCategory::all() {
            assert_eq!(SavingsCategory::from_str(c.as_str()), Some(*c));
        }
    }

    #[test]
    fn cost_model_scales_with_price() {
        std::env::set_var(TOKEN_PRICE_ENV, "6.0");
        assert!((tokens_to_usd(1_000_000) - 6.0).abs() < 1e-9);
        std::env::remove_var(TOKEN_PRICE_ENV);
    }

    #[test]
    fn alert_maps_to_event() {
        let alert = Alert {
            severity: AlertSeverity::Critical,
            alert_type: AlertType::MemoryPressure,
            message: "RAM critical".to_string(),
            ts: Utc::now(),
            metadata: AlertMetadata {
                ram_pct: Some(96.0),
                cpu_pct: None,
                temp_c: None,
                disk_pct: None,
                culprit: None,
                culprit_group: None,
            },
        };
        let ev = from_alert(&alert).expect("should map");
        assert_eq!(ev.category, SavingsCategory::PreventedOomCrash);
        assert_eq!(ev.source, SavingsSource::Detected);
        assert!(ev.tokens_saved > 0);
        assert!(ev.usd_saved > 0.0);
    }

    #[test]
    fn resolved_alert_is_not_a_saving() {
        let alert = Alert {
            severity: AlertSeverity::Resolved,
            alert_type: AlertType::MemoryPressure,
            message: "recovered".to_string(),
            ts: Utc::now(),
            metadata: AlertMetadata {
                ram_pct: None,
                cpu_pct: None,
                temp_c: None,
                disk_pct: None,
                culprit: None,
                culprit_group: None,
            },
        };
        assert!(from_alert(&alert).is_none());
    }
}
