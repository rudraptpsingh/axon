use std::sync::Arc;

use axon_core::{
    alert_dispatch::AlertDispatcher,
    collector::SharedState,
    persistence::{self, DbHandle},
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

// ── MCP Server ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct AxonServer {
    state: SharedState,
    db: DbHandle,
    tool_router: ToolRouter<Self>,
}

impl AxonServer {
    pub fn new(state: SharedState, db: DbHandle) -> Self {
        Self {
            state,
            db,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl AxonServer {
    #[tool(
        description = "Real-time hardware snapshot: CPU usage %, die temperature, RAM used/total, RAM pressure level (normal/warn/critical), disk used/total, disk pressure level (normal/warn/critical), and whether the CPU is thermally throttling."
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
        description = "Identify what is slowing your Mac right now. Returns the top culprit process, impact severity (healthy/degrading/strained/critical), and a concrete fix. Call this when your AI session lags or crashes."
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
        description = "Static machine info: model ID, chip, core count, total RAM, macOS version. Read once at session start to understand host capabilities."
    )]
    async fn system_profile(&self, _p: Parameters<EmptyParams>) -> String {
        let profile = {
            let guard = self.state.lock().unwrap();
            guard.profile.clone()
        };
        let narrative = format!(
            "{} ({}) — {} cores, {:.0}GB RAM, {}.",
            profile.model_id,
            profile.chip,
            profile.core_count,
            profile.ram_total_gb,
            profile.os_version
        );
        let response = McpResponse::success(profile, narrative);
        serde_json::to_string(&response)
            .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
    }

    #[tool(
        description = "Hardware trends over time: CPU, RAM, temperature averages and peaks per interval. Use to detect degradation patterns, justify hardware upgrades, or check if your Mac is getting slower. Accepts optional time_range (default: last_24h) and interval (default: 15m)."
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for AxonServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build()).with_instructions(
            "axon: local hardware intelligence for AI agents. \
                Call process_blame when your session lags. \
                Call hw_snapshot for current system state. \
                Call battery_status before long tasks. \
                Call system_profile once for machine specs. \
                Call hardware_trend for historical CPU/RAM/temp trends.",
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
    format!(
        "CPU {:.0}%, die {}{} RAM {:.1}/{:.0}GB ({} pressure).{}",
        hw.cpu_usage_pct, temp_str, throttle, hw.ram_used_gb, hw.ram_total_gb, pressure, disk_str
    )
}

fn blame_narrative(blame: &ProcessBlame) -> String {
    // Prefer group narrative when multiple processes are grouped
    if let Some(g) = &blame.culprit_group {
        if blame.anomaly_score > 0.1 && g.process_count > 1 {
            return format!(
                "{} ({:.1}GB across {} processes, {:.0}% CPU) — {} {}",
                g.name, g.total_ram_gb, g.process_count, g.total_cpu_pct, blame.impact, blame.fix
            );
        }
    }
    match &blame.culprit {
        Some(p) if blame.anomaly_score > 0.1 => format!(
            "{} (PID {}, {:.0}% CPU, {:.1}GB RAM) — {} {}",
            p.cmd, p.pid, p.cpu_pct, p.ram_gb, blame.impact, blame.fix
        ),
        _ => format!("{} {}", blame.impact, blame.fix),
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
    dispatcher: Arc<AlertDispatcher>,
) -> anyhow::Result<()> {
    let server = AxonServer::new(state.clone(), db.clone());
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
