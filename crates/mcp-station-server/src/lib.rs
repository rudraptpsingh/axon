use mcp_station_core::{
    collector::SharedState,
    types::*,
};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{ServerCapabilities, ServerInfo},
    schemars,
    tool, tool_handler, tool_router,
};

// ── Tool Parameter Types ──────────────────────────────────────────────────────

#[derive(Debug, ::serde::Deserialize, schemars::JsonSchema)]
pub struct EmptyParams {}

// ── MCP Server ────────────────────────────────────────────────────────────────

#[derive(Clone)]
pub struct McpStation {
    state: SharedState,
    tool_router: ToolRouter<Self>,
}

impl McpStation {
    pub fn new(state: SharedState) -> Self {
        Self {
            state,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router(router = tool_router)]
impl McpStation {
    #[tool(
        description = "Real-time hardware snapshot: CPU usage %, die temperature, RAM used/total, RAM pressure level (normal/warn/critical), and whether the CPU is thermally throttling."
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
            None => {
                r#"{"ok":false,"narrative":"Battery information unavailable."}"#.to_string()
            }
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
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for McpStation {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions(
                "mcp-station: local hardware intelligence for AI agents. \
                Call process_blame when your session lags. \
                Call hw_snapshot for current system state. \
                Call battery_status before long tasks. \
                Call system_profile once for machine specs.",
            )
    }
}

// ── Narrative Helpers ─────────────────────────────────────────────────────────

fn hw_narrative(hw: &HwSnapshot) -> String {
    let temp_str = hw
        .die_temp_celsius
        .map(|t| format!("{:.0}°C", t))
        .unwrap_or_else(|| "temp N/A".to_string());
    let throttle = if hw.throttling { " ⚠️ throttling" } else { "" };
    let pressure = match hw.ram_pressure {
        RamPressure::Normal => "normal",
        RamPressure::Warn => "warn",
        RamPressure::Critical => "critical",
    };
    format!(
        "CPU {:.0}%, die {}{} RAM {:.1}/{:.0}GB ({} pressure).",
        hw.cpu_usage_pct, temp_str, throttle, hw.ram_used_gb, hw.ram_total_gb, pressure
    )
}

fn blame_narrative(blame: &ProcessBlame) -> String {
    match &blame.culprit {
        Some(p) if blame.anomaly_score > 0.1 => format!(
            "{} (PID {}, {:.0}% CPU, {:.1}GB RAM) — {} {}",
            p.cmd, p.pid, p.cpu_pct, p.ram_gb, blame.impact, blame.fix
        ),
        _ => format!("{} {}", blame.impact, blame.fix),
    }
}

// ── Public Entry Point ────────────────────────────────────────────────────────

pub async fn run_server(state: SharedState) -> anyhow::Result<()> {
    let server = McpStation::new(state);
    let transport = (tokio::io::stdin(), tokio::io::stdout());
    let running = server.serve(transport).await?;
    running.waiting().await?;
    Ok(())
}
