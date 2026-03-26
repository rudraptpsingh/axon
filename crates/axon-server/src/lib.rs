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
        // Disk fill rate warning: runaway debug-log loop (#16093) or task .output
        // accumulation (#26911, 537GB from one session). Rate >0.1 GB/s = crisis.
        let fill_rate_str = match hw.disk_fill_rate_gb_per_sec {
            Some(r) if r >= 0.5 => format!(
                " [CRITICAL] Disk filling at {:.1} GB/s — runaway write loop likely. \
                 Check: du -sh ~/.claude/debug/ /tmp/claude-*/ and kill the process \
                 writing. This matches infinite logging loop pattern (#16093).",
                r
            ),
            Some(r) if r >= 0.05 => format!(
                " [WARN] Disk filling at {:.0} MB/s — possible task .output accumulation \
                 or debug log growth. Check: du -sh ~/.claude/debug/ /tmp/claude-*/",
                r * 1024.0
            ),
            _ => String::new(),
        };
        format!(
            " Disk {:.1}/{:.0}GB ({:.0}%, {} pressure).{}",
            hw.disk_used_gb, hw.disk_total_gb, disk_pct, disk_pressure_str, fill_rate_str
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
    // Swap pressure: high swap = RAM exhaustion, forces paging (Cowork VM bundle #22543).
    let swap_str = match (hw.swap_used_gb, hw.swap_total_gb) {
        (Some(used), Some(total)) if total > 0.1 => {
            let pct = used / total * 100.0;
            if pct > 50.0 {
                format!(" Swap {:.1}/{:.0}GB ({:.0}%, HIGH — system paging heavily; systemd-oomd may kill processes if sustained >20s).", used, total, pct)
            } else if pct > 10.0 {
                format!(" Swap {:.1}/{:.0}GB ({:.0}%).", used, total, pct)
            } else {
                String::new()
            }
        }
        _ => String::new(),
    };
    // IRQ rate hint: high IRQ with moderate CPU → real I/O; near-zero IRQ with high CPU → spin-loop.
    let irq_str = match hw.irq_per_sec {
        Some(irq) if irq > 50_000 => format!(" IRQ {}/s (I/O-heavy).", irq),
        Some(irq) if hw.cpu_usage_pct > 60.0 && irq < 5_000 => {
            format!(" IRQ {}/s (low — possible spin-loop).", irq)
        }
        _ => String::new(),
    };
    // System FD pool exhaustion: affects all processes system-wide (#11136, Cursor file watcher).
    let fd_str = match hw.system_fd_pct {
        Some(pct) if pct >= 95.0 => format!(
            " [CRITICAL] System FD pool {:.0}% full — any open() will fail with ENFILE. \
             Restart processes leaking inotify watchers immediately: \
             lsof | awk '{{print $2}}' | sort | uniq -c | sort -rn | head",
            pct
        ),
        Some(pct) if pct >= 85.0 => format!(
            " [WARN] System FD pool {:.0}% full — approaching global ENFILE limit. \
             Investigate with: cat /proc/sys/fs/file-nr",
            pct
        ),
        _ => String::new(),
    };
    // OOM freeze risk: no swap + minimal free RAM = Linux hard freeze on next big alloc.
    let oom_str = if hw.oom_freeze_risk == Some(true) {
        " [CRITICAL] OOM freeze risk — MemFree + SwapFree < 64MB with no swap configured. \
         Next large allocation will trigger kernel freeze (not OOM kill). \
         Free RAM immediately or add swap: sudo fallocate -l 4G /swapfile && \
         sudo mkswap /swapfile && sudo swapon /swapfile"
            .to_string()
    } else {
        String::new()
    };
    // Dot-claude size: runaway logs/cache accumulation.
    let dot_claude_str = match hw.dot_claude_size_gb {
        Some(gb) if gb >= 10.0 => format!(
            " [WARN] ~/.claude/ is {:.0}GB — likely runaway debug logs or node_modules cache. \
             Check: du -sh ~/.claude/debug/ ~/.claude/node_modules/ ~/.claude/projects/",
            gb
        ),
        Some(gb) if gb >= 2.0 => format!(" [INFO] ~/.claude/ is {:.1}GB.", gb),
        _ => String::new(),
    };
    // Tmp-claude size: task .output files, napi-rs addons, cowork VM fragments accumulate
    // with no TTL. Observed 537 GB from a single session (#26911).
    let tmp_claude_str = match hw.tmp_claude_size_gb {
        Some(gb) if gb >= 50.0 => format!(
            " [CRITICAL] /tmp/claude-{{uid}}/ is {:.0}GB — task .output files or napi-rs temp \
             addons accumulating with no cleanup (github.com/anthropics/claude-code/issues/26911). \
             Run: rm -rf /tmp/claude-$(id -u)/ to free disk immediately.",
            gb
        ),
        Some(gb) if gb >= 5.0 => format!(
            " [WARN] /tmp/claude-{{uid}}/ is {:.1}GB — task output files accumulating. \
             Check: du -sh /tmp/claude-$(id -u)/ and remove if safe.",
            gb
        ),
        _ => String::new(),
    };
    // Fork bomb / process spawn storm detection.
    let spawn_rate_str = match hw.process_spawn_rate_per_sec {
        Some(rate) if rate > 200.0 => format!(
            " [CRITICAL] Process creation rate {:.0}/s — fork bomb or runaway posix_spawn loop \
             (github.com/anthropics/claude-code/issues/36127, #37490). \
             Run: pkill -f claude && kill %1 to stop the cascade.",
            rate
        ),
        Some(rate) if rate > 50.0 => format!(
            " [WARN] Process creation rate {:.0}/s — possible subprocess storm. \
             Monitor: ps aux | wc -l to check total process count.",
            rate
        ),
        _ => String::new(),
    };
    // MCP server count: too many simultaneous MCP servers drain commit charge.
    let mcp_str = match hw.mcp_server_count {
        Some(n) if n >= 8 => format!(
            " [WARN] {} MCP server processes running — high server count accumulates commit \
             charge per session. Consider restarting Claude to release unused MCP servers.",
            n
        ),
        Some(n) if n >= 4 => format!(" {} MCP servers running.", n),
        _ => String::new(),
    };
    format!(
        "CPU {:.0}% {}, die {}{} RAM {:.1}/{:.0}GB {} ({} pressure).{}{}{}{}{}{}{}{}{}  {} | {}",
        hw.cpu_usage_pct,
        hw.cpu_trend,
        temp_str,
        throttle,
        hw.ram_used_gb,
        hw.ram_total_gb,
        hw.ram_trend,
        pressure,
        disk_str,
        swap_str,
        irq_str,
        fd_str,
        oom_str,
        dot_claude_str,
        tmp_claude_str,
        spawn_rate_str,
        mcp_str,
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

    // Orphaned subprocesses (ex-claude descendants reparented to init, or high-CPU MCP plugins).
    // Common cause: claude exited ungracefully, bun/node MCP servers left pegging CPU.
    // See: github.com/anthropics/claude-code/issues/39170
    if !blame.orphan_pids.is_empty() {
        let pids = blame
            .orphan_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        base.push_str(&format!(
            " [WARN] {} orphaned subprocess(es) consuming CPU (PIDs: {}). \
             Likely MCP plugins from a crashed session. Kill: kill -9 {}",
            blame.orphan_pids.len(),
            pids,
            pids
        ));
    }

    // Crashed claude processes: PIDs tracked last tick that have disappeared.
    // Likely causes: Bun segfault (#21875), OOM kill (#39022), SIGKILL.
    if !blame.crashed_agent_pids.is_empty() {
        let pids = blame
            .crashed_agent_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        base.push_str(&format!(
            " [WARN] Claude PID(s) {} disappeared unexpectedly — likely crashed \
             (Bun segfault, OOM kill, or SIGKILL). Check system logs: journalctl -k | grep -E 'Killed|OOM'",
            pids
        ));
    }

    // Zombie subprocesses: un-reaped children of claude, holding PID slots.
    if !blame.zombie_pids.is_empty() {
        let pids = blame
            .zombie_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        base.push_str(&format!(
            " [WARN] {} zombie subprocess(es) not reaped by parent (PIDs: {}).",
            blame.zombie_pids.len(),
            pids
        ));
    }

    // Parallel agent storm: too many concurrent claude subagents can saturate disk I/O
    // and CPU on resource-constrained machines, causing system lockups before OOM triggers.
    // Observed: 24 agents in 2 min → 17x disk I/O spike → system freeze (#15487).
    {
        let non_orch_count = blame
            .claude_agents
            .iter()
            .filter(|a| !a.is_orchestrator)
            .count();
        if non_orch_count >= 8 {
            let pids: Vec<String> = blame
                .claude_agents
                .iter()
                .filter(|a| !a.is_orchestrator)
                .map(|a| a.pid.to_string())
                .collect();
            base.push_str(&format!(
                " [WARN] {} parallel claude subagents running simultaneously — disk I/O storm risk \
                 (#15487). On limited systems (VPS, low-RAM) this can cause system freeze before OOM. \
                 Consider reducing parallel tasks. PIDs: {}",
                non_orch_count,
                pids.join(" ")
            ));
        }
    }

    // Fast RAM spike detection: runaway allocation from terminal resize (SIGWINCH burst).
    // Seen as 1GB→21GB in 6s in issue #39022 — OOM kill risk within seconds.
    let spiking: Vec<String> = blame
        .claude_agents
        .iter()
        .filter(|a| a.ram_spike == Some(true))
        .map(|a| format!("PID {} ({:.1}GB)", a.pid, a.ram_gb))
        .collect();
    if !spiking.is_empty() {
        base.push_str(&format!(
            " [CRITICAL] Runaway RAM allocation on {} — OOM kill imminent. \
             Stop resizing terminal NOW. If in tmux, stop dragging pane divider. \
             Emergency kill: kill -9 {}",
            spiking.join(", "),
            blame
                .claude_agents
                .iter()
                .filter(|a| a.ram_spike == Some(true))
                .map(|a| a.pid.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    // Orchestrator stall detection (#38932): orchestrator idle for >10min while
    // subagents are still running = Ink render loop throttled, inbox poller starved.
    // Manual keystroke is the only known workaround; axon alerts the agent.
    let orchestrator_idle_min = blame
        .claude_agents
        .iter()
        .find(|a| a.is_orchestrator)
        .and_then(|orch| {
            let idle_secs = orch.uptime_s?;
            // Rough heuristic: if orchestrator cpu < 2% but has been running >5min
            // and there are active subagents, it may be stalled.
            let has_subagents = blame.claude_agents.iter().any(|a| !a.is_orchestrator);
            let low_cpu = orch.cpu_pct < 2.0;
            let long_lived = idle_secs > 300; // 5 min
            if has_subagents && low_cpu && long_lived {
                Some((orch.pid, idle_secs / 60))
            } else {
                None
            }
        });
    if let Some((orch_pid, idle_min)) = orchestrator_idle_min {
        base.push_str(&format!(
            " [WARN] Orchestrator PID {} idle for ~{}min with active subagents — \
             possible Ink render loop stall (#38932). \
             Send a keystroke to the terminal to wake the inbox poller.",
            orch_pid, idle_min
        ));
    }

    // I/O block detection: D-state (uninterruptible wait) for 2+ ticks — WSL2 filesystem
    // bridging, NFS hang, or overlapping heavy I/O causing 1-6min thinking delays (#22855).
    let io_blocked: Vec<String> = blame
        .claude_agents
        .iter()
        .filter(|a| a.suspected_io_block == Some(true))
        .map(|a| format!("PID {} ({:.0}%CPU)", a.pid, a.cpu_pct))
        .collect();
    if !io_blocked.is_empty() {
        base.push_str(&format!(
            " [WARN] Claude {} in D-state (I/O blocking) — filesystem or network stall. \
             On WSL2 this causes 1-6min thinking delays. \
             Check: cat /proc/{}/wchan to see what it's waiting on.",
            io_blocked.join(", "),
            blame
                .claude_agents
                .iter()
                .filter(|a| a.suspected_io_block == Some(true))
                .map(|a| a.pid.to_string())
                .next()
                .unwrap_or_default()
        ));
    }

    // Spin-loop detection for individual claude agents
    let spinning: Vec<String> = blame
        .claude_agents
        .iter()
        .filter(|a| a.suspected_spin_loop == Some(true))
        .map(|a| format!("PID {} ({:.0}% CPU)", a.pid, a.cpu_pct))
        .collect();
    if !spinning.is_empty() {
        base.push_str(&format!(
            " [WARN] Claude spin-loop suspected on {} — high CPU with near-zero IRQ. \
             May be V8 GC runaway or stuck after MCP response. \
             Consider: kill -9 {} and restart.",
            spinning.join(", "),
            blame
                .claude_agents
                .iter()
                .filter(|a| a.suspected_spin_loop == Some(true))
                .map(|a| a.pid.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        ));
    }

    // GC pressure warnings per claude process.
    // Bun/Node runtime accumulates render buffer state; high RAM → GC thrashing.
    // Known fix: /clear resets buffer (2GB → 160MB, CPU 98% → 0% instantly).
    // Duplicate session ID detection (#39151): multiple claude instances resuming
    // the same session file causes interleaved writes, context corruption, data loss.
    {
        let mut seen: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
        for agent in &blame.claude_agents {
            if let Some(sid) = agent.session_id.as_deref() {
                if let Some(&other_pid) = seen.get(sid) {
                    base.push_str(&format!(
                        " [WARN] Session {} open in multiple claude instances (PIDs: {} and {}). \
                         Concurrent access corrupts session JSONL — kill one: kill {}",
                        &sid[..sid.len().min(12)],
                        other_pid,
                        agent.pid,
                        agent.pid
                    ));
                } else {
                    seen.insert(sid, agent.pid);
                }
            }
        }
    }

    // Browser extension CPU warning: Chrome/Edge extension processes can peg
    // a single helper at 65% CPU. Detect Browser culprit + high CPU + helper in name.
    // See: github.com/anthropics/claude-code/issues/37544
    if matches!(blame.culprit_category, CulpritCategory::Browser) {
        let high_cpu_browser = blame.culprit.as_ref().is_some_and(|p| p.cpu_pct > 50.0);
        let is_helper = blame.culprit.as_ref().is_some_and(|p| {
            let cmd = p.cmd.to_lowercase();
            cmd.contains("helper") || cmd.contains("renderer") || cmd.contains("worker")
        });
        if high_cpu_browser && is_helper {
            base.push_str(
                " [WARN] Browser helper process at high CPU — likely a browser extension \
                 spin-loop. If you have the Claude in Chrome extension installed, \
                 try disabling it (known 65% CPU bug #37544).",
            );
        }
    }

    for agent in &blame.claude_agents {
        let uptime_str = agent
            .uptime_s
            .map(|s| {
                if s >= 3600 {
                    format!(" ({}h session)", s / 3600)
                } else {
                    format!(" ({}m session)", s / 60)
                }
            })
            .unwrap_or_default();
        match agent.gc_pressure.as_deref() {
            Some("critical") => base.push_str(&format!(
                " [CRITICAL] PID {} RAM {:.1}GB{} — Bun GC thrashing imminent. \
                 Run /clear NOW to drop RAM and stop CPU spin (known fix for issue #22509).",
                agent.pid, agent.ram_gb, uptime_str
            )),
            Some("warn") => base.push_str(&format!(
                " [WARN] PID {} RAM {:.1}GB{} — session render buffer growing. \
                 Run /clear if CPU starts climbing (prevents GC thrash at ~2GB).",
                agent.pid, agent.ram_gb, uptime_str
            )),
            Some("accumulating") => base.push_str(&format!(
                " [INFO] PID {} RAM {:.1}GB{} — long session with growing RAM. \
                 Consider /clear proactively to avoid GC pressure later.",
                agent.pid, agent.ram_gb, uptime_str
            )),
            _ => {}
        }

        // Compacting-hang hint: very long session + critical GC pressure = likely stuck
        // context-compaction operation (no timeout, can spin at >100% CPU for hours).
        // See: github.com/anthropics/claude-code/issues/11377.
        if agent.gc_pressure.as_deref() == Some("critical")
            && agent.uptime_s.is_some_and(|s| s > 8 * 3600)
        {
            base.push_str(&format!(
                " [WARN] PID {} has been running {}h with critical RAM — possible compacting \
                 hang (context compression that never finishes). If unresponsive, use: \
                 kill -9 {}",
                agent.pid,
                agent.uptime_s.unwrap_or(0) / 3600,
                agent.pid
            ));
        }

        // VSZ/RSS ratio anomaly: V8 heap fragmentation or 60Hz mmap/munmap thrash loop.
        // Manifests as 50-80% CPU while idle, VSZ 73-85 GB with ~600 MB RSS.
        // See: github.com/anthropics/claude-code/issues/18280.
        if agent.suspected_alloc_thrash == Some(true) {
            base.push_str(&format!(
                " [WARN] PID {} has abnormal VSZ/RSS ratio — V8 heap fragmentation or \
                 memory-allocation thrashing (60Hz mmap loop). Expect 50-80% idle CPU. \
                 Restart claude to reset V8 heap layout.",
                agent.pid
            ));
        }

        // File descriptor leak: fs.watch() / inotify watchers not cleaned up.
        // Grows unboundedly until EMFILE crashes the process.
        // See: github.com/anthropics/claude-code/issues/11136.
        if agent.fd_leak == Some(true) {
            base.push_str(&format!(
                " [WARN] PID {} has a large open-file-descriptor table (FDSize > 4096) — \
                 likely fs.watch/inotify watcher leak. Will crash with EMFILE when ulimit \
                 is reached. Restart claude now to recover; reinstall plugins if recurring.",
                agent.pid
            ));
        }

        // Child churn rate: rapid subprocess spawn/reap = zombie storm (#34092).
        // 185 zombies/sec caused RSS 400MB → 17GB in one session.
        if let Some(rate) = agent.child_churn_rate_per_sec {
            base.push_str(&format!(
                " [WARN] PID {} spawning+reaping children at {:.0}/s — zombie storm pattern. \
                 statusLine render bug (#34092) produced 185/s and RSS grew 400MB→17GB. \
                 Run: kill {} and restart. Check for recursive render or tool-call loops.",
                agent.pid, rate, agent.pid
            ));
        }

        // I/O read polling loop: repeated binary re-reads wasting disk bandwidth.
        // See: github.com/anthropics/claude-code/issues/22543.
        if let Some(rate) = agent.io_read_mb_per_sec {
            base.push_str(&format!(
                " [WARN] PID {} reading {:.0} MB/s from disk with low CPU — likely \
                 polling loop re-reading a large file repeatedly (cowork-svc pattern: \
                 213MB binary every 2s). Check lsof -p {} for the hot file path.",
                agent.pid, rate, agent.pid
            ));
        }

        // Idle CPU spin: sustained CPU burn with no real work (futex/pread loop).
        if let Some(secs) = agent.idle_cpu_spin_secs {
            base.push_str(&format!(
                " [WARN] PID {} has been spinning at >30% CPU for {}s with no child \
                 activity and minimal I/O — likely a userspace poll/futex busy-wait loop. \
                 Check: strace -p {} -e trace=pread64,futex,poll -c for 5s.",
                agent.pid, secs, agent.pid
            ));
        }

        // RSS growth rate: early warning before gc_pressure threshold.
        // See: github.com/anthropics/claude-code/issues/31511, #33118.
        if let Some(rate) = agent.rss_growth_rate_mb_per_hr {
            if rate > 300.0 {
                base.push_str(&format!(
                    " [CRITICAL] PID {} RSS growing at {:.0} MB/hr — crash trajectory. \
                     Likely cause: fetch Response stream bodies not cancelled before GC \
                     (confirmed root cause in github.com/anthropics/claude-code/issues/33874). \
                     Run /clear now or restart claude before the process is killed by OOM.",
                    agent.pid, rate
                ));
            } else if rate > 50.0 {
                base.push_str(&format!(
                    " [WARN] PID {} RSS growing at {:.0} MB/hr — early leak signal. \
                     Monitor: if growth continues, run /clear to reset session buffers.",
                    agent.pid, rate
                ));
            }
        }

        // Large session file: synchronous full load causes infinite hang (#21022).
        if let Some(mb) = agent.large_session_file_mb {
            base.push_str(&format!(
                " [WARN] PID {} session file is {:.0}MB — files > 40MB cause synchronous \
                 full-load hangs (infinite thinking spin, no CPU activity). \
                 Archive old sessions: ls -lh ~/.claude/projects/",
                agent.pid, mb
            ));
        }

        // Bun crash trajectory: uptime + growth rate predict imminent mimalloc OOM.
        if agent.bun_crash_trajectory == Some(true) {
            base.push_str(&format!(
                " [CRITICAL] PID {} is on a crash trajectory (>4h uptime + rapid RSS growth). \
                 mimalloc OOM crash expected within 1-2h (#21875, #29192). \
                 Save work and restart claude now to avoid data loss.",
                agent.pid
            ));
        }

        // Zombie child accumulation per PID (complement to child_churn_rate).
        if let Some(count) = agent.zombie_child_count {
            if count >= 10 {
                base.push_str(&format!(
                    " [WARN] PID {} has {} zombie children — parent not reaping fast enough. \
                     Zombie PID slots accumulate; fork() fails when table fills.",
                    agent.pid, count
                ));
            }
        }

        // Agent stall: process alive but doing nothing for >2 min.
        // Detects stalled API calls, hung IPC, frozen tool execution.
        if let Some(secs) = agent.agent_stall_secs {
            if secs > 300 {
                base.push_str(&format!(
                    " [CRITICAL] PID {} stalled for {:.0} min — near-zero CPU with no I/O or child \
                     activity. Likely stalled API connection or hung IPC socket \
                     (github.com/anthropics/claude-code/issues/25979, #37521). \
                     Kill PID {} and restart the session.",
                    agent.pid, secs as f64 / 60.0, agent.pid
                ));
            } else {
                base.push_str(&format!(
                    " [WARN] PID {} idle for {:.0} min with no progress — possible stalled API call \
                     or hung tool execution (#38258). Monitor; kill if no recovery within 5 min.",
                    agent.pid, secs as f64 / 60.0
                ));
            }
        }

        // Session file growth rate: infers context window burn.
        if let Some(rate) = agent.session_file_growth_mb_per_hr {
            if rate > 500.0 {
                base.push_str(&format!(
                    " [CRITICAL] PID {} session file growing at {:.0} MB/hr — unbounded token \
                     consumption or tool fan-out loop (github.com/anthropics/claude-code/issues/36727). \
                     Run /compact or /clear NOW to prevent crash.",
                    agent.pid, rate
                ));
            } else {
                base.push_str(&format!(
                    " [WARN] PID {} session file growing at {:.0} MB/hr — context burning fast. \
                     Consider /compact to reduce session size before it causes load hangs (#22265).",
                    agent.pid, rate
                ));
            }
        }
    }

    // Stale session summary (blame-level, not per-agent).
    if let Some(count) = blame.stale_session_count {
        base.push_str(&format!(
            " [INFO] {} stale claude session(s) with >200MB RAM and >24h uptime. \
             These are invisible wait states. List: ps aux | grep claude | awk '$2>86400'",
            count
        ));
    }

    // Broad orphan count (includes idle orphans not in orphan_pids).
    if let Some(count) = blame.subagent_orphan_count_total {
        if count > blame.orphan_pids.len() as u32 {
            base.push_str(&format!(
                " [INFO] {} orphaned claude/bun processes (PPID=1) including idle ones. \
                 Check: ps --ppid 1 -o pid,comm,rss | grep -E 'bun|node|claude'",
                count
            ));
        }
    }

    // Background bash shell leak detection.
    if let Some(count) = blame.background_bash_count {
        if count > 20 {
            base.push_str(&format!(
                " [CRITICAL] {} background bash shells owned by claude — shells leaking on process \
                 death (github.com/anthropics/claude-code/issues/38927, #32183). \
                 Kill leaked shells: pkill -P $(pgrep claude) bash",
                count
            ));
        } else if count > 10 {
            base.push_str(&format!(
                " [WARN] {} background bash shells owned by claude — possible shell leak. \
                 Check: pstree -p $(pgrep -o claude) | grep bash",
                count
            ));
        }
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
        trend.buckets.iter().map(|b| b.avg_cpu_pct).sum::<f64>() / trend.buckets.len() as f64
    };

    let ram_overall: f64 = if trend.buckets.is_empty() {
        0.0
    } else {
        trend.buckets.iter().map(|b| b.avg_ram_gb).sum::<f64>() / trend.buckets.len() as f64
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
    let vram_growth_str = match gpu.vram_growth_mb_per_hr {
        Some(rate) if rate > 500.0 => format!(
            " [WARN] GPU VRAM growing at {:.0} MB/hr while GPU is idle — \
             IOAccelerator non-reclaimable memory accumulation across sessions \
             (github.com/anthropics/claude-code/issues/35804). \
             Restart Claude or relaunch the GPU process to reclaim.",
            rate
        ),
        Some(rate) if rate > 100.0 => format!(
            " [INFO] GPU VRAM growing at {:.0} MB/hr — monitor for accumulation (#35804).",
            rate
        ),
        _ => String::new(),
    };
    format!(
        "GPU {}{}: util {}, {}.{}{}",
        model_str, cores_str, util, vram_str, hang_str, vram_growth_str
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
