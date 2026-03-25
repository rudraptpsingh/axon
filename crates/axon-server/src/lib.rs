#[cfg(feature = "dashboard")]
pub mod dashboard;

use std::sync::Arc;

use axon_core::{
    alert_dispatch::AlertDispatcher,
    collector::SharedState,
    persistence::{self, DbHandle},
    ring_buffer::SnapshotRing,
    types::*,
};
use rmcp::{
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{LoggingLevel, LoggingMessageNotificationParam, ServerCapabilities, ServerInfo},
    schemars,
    service::Peer,
    tool, tool_handler, tool_router, RoleServer, ServerHandler, ServiceExt,
};
use tokio::time::{interval, Duration};

// ── Tool Parameter Types ──────────────────────────────────────────────────────

#[derive(Debug, ::serde::Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

#[derive(Debug, ::serde::Deserialize, schemars::JsonSchema)]
pub struct TrendParams {
    #[schemars(
        description = "Time window: last_1h, last_6h, last_24h, last_7d, last_30d (default: last_24h)"
    )]
    pub time_range: Option<String>,
    #[schemars(description = "Bucket interval: 1m, 5m, 15m, 1h, 1d (default: 15m)")]
    pub interval: Option<String>,
}

#[derive(Debug, ::serde::Deserialize, schemars::JsonSchema)]
pub struct SessionHealthParams {
    #[schemars(
        description = "ISO 8601 timestamp to start from (e.g. 2026-03-22T10:00:00Z). Defaults to 1 hour ago if omitted."
    )]
    pub since: Option<String>,
}

// ── MCP Server ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AxonServer {
    state: SharedState,
    db: DbHandle,
    ring: SnapshotRing,
    tool_router: ToolRouter<Self>,
}

impl AxonServer {
    pub fn new(state: SharedState, db: DbHandle, ring: SnapshotRing) -> Self {
        Self {
            state,
            db,
            ring,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl AxonServer {
    #[tool(
        description = "Real-time hardware snapshot with headroom assessment. Returns CPU %, die temperature, RAM/disk pressure levels, thermal throttling status, and a headroom field (adequate/limited/insufficient) that tells you whether it is safe to start a heavy task like compilation, test runs, or code generation. Call BEFORE starting resource-intensive work."
    )]
    async fn hw_snapshot(&self, _p: Parameters<EmptyParams>) -> String {
        let hw = {
            let guard = self.state.lock().unwrap();
            guard.hw.clone()
        };
        let narrative = hw_narrative(&hw);
        let response = McpResponse::success(hw, narrative);
        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
    }

    #[tool(
        description = "Identify what is slowing this machine right now. Returns the top culprit process (or process group), impact severity (healthy/degrading/strained/critical), anomaly type, and a concrete fix suggestion. Also detects when multiple AI agent instances (Claude, Cursor, Windsurf) are accumulating silently. Call when sessions lag, builds fail with OOM, or system feels slow."
    )]
    async fn process_blame(&self, _p: Parameters<EmptyParams>) -> String {
        let blame = {
            let guard = self.state.lock().unwrap();
            guard.blame.clone()
        };
        let narrative = blame_narrative(&blame);
        let response = McpResponse::success(blame, narrative);
        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
    }

    #[tool(
        description = "Battery level, charging status, and estimated time remaining. Call before starting long agentic tasks to avoid running out of power mid-work."
    )]
    async fn battery_status(&self, _p: Parameters<EmptyParams>) -> String {
        let battery = {
            let guard = self.state.lock().unwrap();
            guard.battery.clone()
        };
        match battery {
            Some(b) => {
                let narrative = b.narrative.clone();
                let response = McpResponse::success(b, narrative);
                serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
            }
            None => r#"{"ok":false,"narrative":"Battery information unavailable."}"#.to_string(),
        }
    }

    #[tool(
        description = "Static machine info: model ID, chip/CPU, core count, total RAM, OS version. Read once at session start to understand host capabilities and tailor task parallelism (e.g. cargo build -j based on core count, batch sizes based on available RAM)."
    )]
    async fn system_profile(&self, _p: Parameters<EmptyParams>) -> String {
        let profile = {
            let guard = self.state.lock().unwrap();
            guard.profile.clone()
        };
        let mut narrative = format!(
            "{} ({}) — {} cores, {:.0}GB RAM, {}.",
            profile.model_id,
            profile.chip,
            profile.core_count,
            profile.ram_total_gb,
            profile.os_version
        );
        for w in &profile.startup_warnings {
            narrative.push_str(&format!(" [WARN] {}", w));
        }
        let response = McpResponse::success(profile, narrative);
        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
    }

    #[tool(
        description = "Hardware trends over time: CPU, RAM, temperature averages and peaks per interval. Use to detect degradation patterns, justify hardware upgrades, or check if the machine is getting slower. Accepts optional time_range (default: last_24h) and interval (default: 15m)."
    )]
    async fn hardware_trend(&self, params: Parameters<TrendParams>) -> String {
        let range_str = params.0.time_range.as_deref().unwrap_or("last_24h");
        let interval_str = params.0.interval.as_deref().unwrap_or("15m");

        let range_secs = match persistence::parse_time_range(range_str) {
            Some(s) => s,
            None => {
                return format!(
                    "{{\"ok\":false,\"error\":\"Invalid time_range '{}'. Use: last_1h, last_6h, last_24h, last_7d, last_30d\"}}",
                    range_str
                );
            }
        };
        let bucket_secs = match persistence::parse_interval(interval_str) {
            Some(s) => s,
            None => {
                return format!(
                    "{{\"ok\":false,\"error\":\"Invalid interval '{}'. Use: 1m, 5m, 15m, 1h, 1d\"}}",
                    interval_str
                );
            }
        };

        // Fast path: use the ring buffer for short windows (<=1h).
        // The ring holds ~1h of data at 2s resolution — much higher fidelity
        // than the DB (10s resolution) and avoids disk I/O entirely.
        if range_secs <= 3600 {
            if let Some(trend) = self.ring.hardware_trend(range_secs, bucket_secs) {
                let narrative = trend_narrative(&trend, range_str);
                let response = McpResponse::success(trend, narrative);
                return serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e));
            }
        }

        // Slow path: DB for longer windows or when ring has insufficient data.
        match persistence::query_trend(&self.db, range_secs, bucket_secs) {
            Ok(trend) => {
                let narrative = trend_narrative(&trend, range_str);
                let response = McpResponse::success(trend, narrative);
                serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
            }
            Err(e) => {
                format!("{{\"ok\":false,\"error\":\"{}\"}}", e)
            }
        }
    }

    #[tool(
        description = "Real-time GPU snapshot. Returns GPU utilization %, tiler and renderer stage utilization, VRAM in use and allocated (bytes), cumulative GPU hang/reset count, model name, and core count. macOS (ioreg), Linux (nvidia-smi/AMD sysfs), Windows (nvidia-smi/GPU perf counters + WMI). Returns null fields when no GPU is detected. Call before GPU-heavy workloads (ML inference, 3D rendering, video encoding) to check GPU headroom and detect driver crashes."
    )]
    async fn gpu_snapshot(&self, _p: Parameters<EmptyParams>) -> String {
        let gpu = {
            let guard = self.state.lock().unwrap();
            guard.gpu.clone()
        };
        match gpu {
            Some(g) => {
                let narrative = gpu_narrative(&g);
                let response = McpResponse::success(g, narrative);
                serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
            }
            None => r#"{"ok":false,"narrative":"GPU metrics unavailable on this platform."}"#
                .to_string(),
        }
    }

    #[tool(
        description = "Session health summary since a given timestamp. Returns worst impact level, worst anomaly type, alert count, throttle events, average and peak CPU/RAM/temperature. Use at the end of long sessions or periodically to detect gradual degradation that edge-triggered alerts may miss."
    )]
    async fn session_health(&self, params: Parameters<SessionHealthParams>) -> String {
        let since = params
            .0
            .since
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
            .unwrap_or_else(|| chrono::Utc::now() - chrono::Duration::hours(1));

        // Fast path: use the in-memory ring buffer when it covers the window.
        // This avoids SQLite I/O for the common case (default 1h, ring holds ~30min).
        if let Some(mut health) = self.ring.session_health(since) {
            // The ring doesn't track alerts — get the count from DB (cheap single-row query).
            if let Ok(count) = persistence::query_alert_count(&self.db, since) {
                health.alert_count = count;
            }
            let narrative = session_health_narrative(&health);
            let response = McpResponse::success(health, narrative);
            return serde_json::to_string(&response)
                .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e));
        }

        // Slow path: ring doesn't cover the window — fall back to SQLite.
        match persistence::query_session_health(&self.db, since) {
            Ok(health) => {
                let narrative = session_health_narrative(&health);
                let response = McpResponse::success(health, narrative);
                serde_json::to_string(&response)
                    .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
            }
            Err(e) => {
                format!("{{\"ok\":false,\"error\":\"{}\"}}", e)
            }
        }
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AxonServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "axon: zero-cloud local hardware intelligence for AI coding agents. \
                Recommended workflow: \
                1. Call system_profile once at session start for machine specs (core count, RAM). \
                2. Call hw_snapshot before heavy tasks (builds, test runs, large edits) -- check the headroom field. \
                   If headroom=insufficient, warn the user or defer the task. \
                3. Call process_blame when sessions lag, builds OOM, or the system feels slow. \
                   It detects agent accumulation (multiple Claude/Cursor instances). \
                4. Call battery_status before long agentic tasks on laptops. \
                5. Call hardware_trend for degradation patterns over time. \
                6. Call session_health at end of long sessions for a retrospective summary. \
                7. Call gpu_snapshot before GPU-heavy workloads (ML inference, video encoding, 3D rendering) \
                   to check GPU utilization and VRAM pressure. Also check recovery_count for GPU driver crashes.",
        )
    }
}

// ── Narrative Helpers ─────────────────────────────────────────────────────────

pub fn hw_narrative_pub(hw: &HwSnapshot) -> String {
    hw_narrative(hw)
}

pub fn blame_narrative_pub(blame: &ProcessBlame) -> String {
    blame_narrative(blame)
}

pub fn session_health_narrative_pub(health: &SessionHealth) -> String {
    session_health_narrative(health)
}

pub fn gpu_narrative_pub(gpu: &GpuSnapshot) -> String {
    gpu_narrative(gpu)
}

pub fn trend_narrative_pub(trend: &TrendData, range: &str) -> String {
    trend_narrative(trend, range)
}

fn hw_narrative(hw: &HwSnapshot) -> String {
    let temp_str = hw
        .die_temp_celsius
        .map(|t| format!("{:.0}°C", t))
        .unwrap_or_else(|| "temp N/A".to_string());
    let throttle = if hw.throttling { " [THROTTLING]" } else { "" };
    let pressure = match hw.ram_pressure {
        RamPressure::Normal => "normal",
        RamPressure::Warn => "warn",
        RamPressure::Critical => "critical",
    };
    let disk_str = if hw.disk_total_gb > 0.0 {
        let disk_pct = hw.disk_used_gb / hw.disk_total_gb * 100.0;
        let disk_pressure_str = match hw.disk_pressure {
            DiskPressure::Normal => "normal",
            DiskPressure::Warn => "warn",
            DiskPressure::Critical => "critical",
        };
        format!(
            " Disk {:.1}/{:.0}GB ({:.0}%, {} pressure).",
            hw.disk_used_gb, hw.disk_total_gb, disk_pct, disk_pressure_str
        )
    } else {
        String::new()
    };
    let headroom_str = match hw.headroom {
        HeadroomLevel::Adequate => "Headroom: adequate.".to_string(),
        HeadroomLevel::Limited => {
            format!("Headroom: limited ({}).", hw.headroom_reason)
        }
        HeadroomLevel::Insufficient => {
            format!(
                "Headroom: INSUFFICIENT ({}) -- defer heavy tasks.",
                hw.headroom_reason
            )
        }
    };
    format!(
        "CPU {:.0}% {}, die {}{} RAM {:.1}/{:.0}GB {} ({} pressure).{} {} | {}",
        hw.cpu_usage_pct,
        hw.cpu_trend,
        temp_str,
        throttle,
        hw.ram_used_gb,
        hw.ram_total_gb,
        hw.ram_trend,
        pressure,
        disk_str,
        headroom_str,
        hw.one_liner,
    )
}

fn blame_narrative(blame: &ProcessBlame) -> String {
    // Prefer group narrative when multiple processes are grouped
    let mut base = if let Some(g) = &blame.culprit_group {
        if blame.anomaly_score > 0.1 && g.process_count > 1 {
            format!(
                "[{}] {} ({:.1}GB across {} processes, {:.0}% CPU) — {} {} [urgency: {}]",
                blame.culprit_category,
                g.name,
                g.total_ram_gb,
                g.process_count,
                g.total_cpu_pct,
                blame.impact,
                blame.fix,
                blame.urgency,
            )
        } else {
            blame_narrative_fallback(blame)
        }
    } else {
        blame_narrative_fallback(blame)
    };

    // Append stale-instance warning if siblings detected
    if !blame.stale_axon_pids.is_empty() {
        let pids = blame
            .stale_axon_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        base.push_str(&format!(
            " [WARN] {} stale axon instance(s) running (PIDs: {}). Kill: kill {}",
            blame.stale_axon_pids.len(),
            pids,
            pids
        ));
    }

    base
}

fn blame_narrative_fallback(blame: &ProcessBlame) -> String {
    match &blame.culprit {
        Some(p) if blame.anomaly_score > 0.1 => format!(
            "[{}] {} (PID {}, {:.0}% CPU, {:.1}GB RAM) — {} {} [urgency: {}]",
            blame.culprit_category,
            p.cmd,
            p.pid,
            p.cpu_pct,
            p.ram_gb,
            blame.impact,
            blame.fix,
            blame.urgency,
        ),
        _ => {
            if blame.fix == blame.impact || blame.fix == "No action needed." {
                blame.impact.clone()
            } else {
                format!("{} {}", blame.impact, blame.fix)
            }
        }
    }
}

fn trend_narrative(trend: &TrendData, range: &str) -> String {
    if trend.total_snapshots == 0 {
        return format!(
            "No data available for {}. Start axon and wait for snapshots to accumulate.",
            range
        );
    }

    let total_anomalies: u32 = trend.buckets.iter().map(|b| b.anomaly_count).sum();
    let total_throttles: u32 = trend.buckets.iter().map(|b| b.throttle_count).sum();

    let cpu_overall: f64 = if trend.buckets.is_empty() {
        0.0
    } else {
        trend.buckets.iter().map(|b| b.cpu_avg).sum::<f64>() / trend.buckets.len() as f64
    };

    let ram_overall: f64 = if trend.buckets.is_empty() {
        0.0
    } else {
        trend.buckets.iter().map(|b| b.ram_avg).sum::<f64>() / trend.buckets.len() as f64
    };

    let mut parts = vec![
        format!("CPU {} (avg {:.0}%)", trend.trend_direction, cpu_overall),
        format!("RAM avg {:.1}GB", ram_overall),
    ];

    if total_anomalies > 0 {
        parts.push(format!("{} anomalies", total_anomalies));
    }
    if total_throttles > 0 {
        parts.push(format!("{} throttle events", total_throttles));
    }

    format!(
        "{} over {} ({} snapshots).",
        parts.join(", "),
        range,
        trend.total_snapshots
    )
}

fn gpu_narrative(gpu: &GpuSnapshot) -> String {
    if !gpu.detected {
        return "No GPU detected on this system. \
                nvidia-smi not found and no DRM sysfs device present."
            .to_string();
    }
    let util = gpu
        .utilization_pct
        .map(|v| format!("{:.0}%", v))
        .unwrap_or_else(|| "N/A".to_string());
    let vram_str = match (gpu.vram_used_bytes, gpu.vram_alloc_bytes) {
        (Some(used), Some(alloc)) => format!(
            "{:.0}/{:.0}MB VRAM",
            used as f64 / 1_048_576.0,
            alloc as f64 / 1_048_576.0
        ),
        (Some(used), None) => format!("{:.0}MB VRAM used", used as f64 / 1_048_576.0),
        _ => "VRAM N/A".to_string(),
    };
    let model_str = gpu.model.as_deref().unwrap_or("unknown GPU");
    let cores_str = gpu
        .core_count
        .map(|c| format!(" ({} cores)", c))
        .unwrap_or_default();
    let hang_str = match gpu.recovery_count {
        Some(0) | None => String::new(),
        Some(n) => format!(" [WARN: {} GPU hang(s)]", n),
    };
    format!(
        "GPU {}{}: util {}, {}.{}",
        model_str, cores_str, util, vram_str, hang_str
    )
}

fn session_health_narrative(health: &SessionHealth) -> String {
    if health.snapshot_count == 0 {
        return format!(
            "No data since {}. Start axon and wait for snapshots.",
            health.since.format("%H:%M")
        );
    }

    let impact_str = match health.worst_impact_level {
        ImpactLevel::Healthy => "healthy",
        ImpactLevel::Degrading => "degrading",
        ImpactLevel::Strained => "strained",
        ImpactLevel::Critical => "CRITICAL",
    };

    let mut parts = vec![
        format!(
            "Since {}: {} snapshots, worst impact: {}",
            health.since.format("%H:%M"),
            health.snapshot_count,
            impact_str
        ),
        format!(
            "CPU avg {:.0}% (peak {:.0}%), RAM avg {:.1}GB (peak {:.1}GB)",
            health.avg_cpu_pct, health.peak_cpu_pct, health.avg_ram_gb, health.peak_ram_gb
        ),
    ];

    if health.alert_count > 0 {
        parts.push(format!("{} alerts fired", health.alert_count));
    }
    if health.throttle_event_count > 0 {
        parts.push(format!("{} throttle events", health.throttle_event_count));
    }
    if let Some(t) = health.peak_temp_celsius {
        parts.push(format!("peak temp {:.0}C", t));
    }

    parts.join(". ") + "."
}

// ── Alert Notification Sender ─────────────────────────────────────────────────

async fn alert_sender(
    state: SharedState,
    peer: Peer<RoleServer>,
    dispatcher: Arc<AlertDispatcher>,
) {
    let mut ticker = interval(Duration::from_secs(2));
    loop {
        ticker.tick().await;

        let alerts = {
            let mut guard = state.lock().unwrap();
            std::mem::take(&mut guard.pending_alerts)
        };

        for alert in &alerts {
            // Alert was already persisted by the collector. Send webhooks + check MCP flag.
            let send_via_mcp = dispatcher.dispatch_webhooks_only(alert).await;

            if send_via_mcp {
                let level = match alert.severity {
                    AlertSeverity::Warning => LoggingLevel::Warning,
                    AlertSeverity::Critical => LoggingLevel::Critical,
                    AlertSeverity::Resolved => LoggingLevel::Info,
                };

                let param = LoggingMessageNotificationParam {
                    level,
                    logger: Some("axon".to_string()),
                    data: serde_json::json!(alert.message),
                };

                if let Err(e) = peer.notify_logging_message(param).await {
                    tracing::debug!("alert send failed (client may have disconnected): {}", e);
                    return;
                }
            }
        }
    }
}

// ── Public Entry Point ────────────────────────────────────────────────────────

pub async fn run_server(
    state: SharedState,
    db: DbHandle,
    ring: SnapshotRing,
    dispatcher: Arc<AlertDispatcher>,
) -> anyhow::Result<()> {
    let server = AxonServer::new(state.clone(), db.clone(), ring);
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let running = server.serve(transport).await?;

    // Spawn alert notification sender using the peer
    let peer = running.peer().clone();
    tokio::spawn(alert_sender(state.clone(), peer, dispatcher.clone()));

    running.waiting().await?;

    // Drain any pending alerts that the alert_sender task may not have processed yet.
    // Alerts are already persisted by the collector; this flushes webhook dispatches.
    let remaining = {
        let mut guard = state.lock().unwrap();
        std::mem::take(&mut guard.pending_alerts)
    };
    for alert in &remaining {
        dispatcher.dispatch_webhooks_only(alert).await;
    }

    Ok(())
}
