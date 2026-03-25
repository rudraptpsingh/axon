use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use axon_core::alert_config;
use axon_core::alert_dispatch::AlertDispatcher;
use axon_core::collector::{build_system_profile, start_collector, AppState};
use axon_core::persistence;
use axon_core::ring_buffer::SnapshotRing;
use axon_server::run_server;

// ── CLI Definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "axon")]
#[command(version)]
#[command(
    about = "Local hardware intelligence for AI-powered developers",
    long_about = "axon gives AI coding agents (Cursor, Claude Code, Windsurf) \
    real-time awareness of your Mac's hardware: CPU load, die temperature, memory pressure, \
    and which process is responsible -- without sending a single byte off-device."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Args, Debug, Clone, Default)]
struct ServeArgs {
    /// Directory containing alert-dispatch.json (overrides ~/.config/axon; same as AXON_CONFIG_DIR)
    #[arg(long)]
    config_dir: Option<PathBuf>,
    /// Add or replace a webhook channel: ID=http://host/path (repeatable)
    #[arg(long = "alert-webhook", value_name = "ID=URL")]
    alert_webhook: Vec<String>,
    /// Filter for a channel: channel_id.severity=critical or channel_id.types=a,b (repeatable)
    #[arg(long = "alert-filter", value_name = "CHANNEL.KEY=VALUE")]
    alert_filter: Vec<String>,
    /// Start the web dashboard on port 7670
    #[cfg(feature = "dashboard")]
    #[arg(long)]
    dashboard: bool,
    /// Dashboard port (default: 7670)
    #[cfg(feature = "dashboard")]
    #[arg(long, default_value = "7670")]
    dashboard_port: u16,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP stdio server (used in claude_desktop_config.json)
    Serve(ServeArgs),

    /// One-shot diagnosis: collect 4 seconds of data, then print the culprit
    Diagnose,

    /// Print current hardware snapshot as JSON
    Status,

    /// Call an MCP tool directly and print the JSON response (e.g. process_blame, hw_snapshot)
    Query {
        /// Tool name: process_blame | hw_snapshot | battery_status | system_profile | session_health | gpu_snapshot | hardware_trend
        #[arg(value_name = "TOOL")]
        tool: String,
    },

    /// Configure AI agents to use axon (all detected agents if no target given)
    Setup {
        /// Target client: claude-desktop | claude-code | cursor | vscode (omit to configure all)
        #[arg(value_name = "TARGET")]
        target: Option<String>,
        /// Show which agents currently have axon configured
        #[arg(long)]
        list: bool,
    },

    /// Remove axon from AI agent configs and delete local data (reverse of setup)
    Uninstall {
        /// Target client: claude-desktop | claude-code | cursor | vscode (omit to remove from all)
        #[arg(value_name = "TARGET")]
        target: Option<String>,
    },
}

// ── Entry Point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr only -- stdout is reserved for MCP JSON-RPC
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let cli = Cli::parse();

    match cli.command {
        None => run_serve(ServeArgs::default()).await,
        Some(Commands::Serve(args)) => run_serve(args).await,
        Some(Commands::Diagnose) => run_diagnose().await,
        Some(Commands::Status) => run_status().await,
        Some(Commands::Query { tool }) => run_query(&tool).await,
        Some(Commands::Setup { target, list }) => {
            if list {
                run_setup_list()
            } else {
                run_setup(target.as_deref())
            }
        }
        Some(Commands::Uninstall { target }) => run_uninstall(target.as_deref()),
    }
}

// ── Command Handlers ──────────────────────────────────────────────────────────

async fn run_serve(args: ServeArgs) -> Result<()> {
    tracing::info!("axon starting (stdio transport)");
    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let db_path = persistence::default_db_path()?;
    let db = persistence::open(db_path)?;

    // Load alert dispatch configuration (file + CLI overrides)
    let mut config = alert_config::load_config(args.config_dir.as_ref());
    let webhooks: Vec<(String, String)> = args
        .alert_webhook
        .iter()
        .map(|s| alert_config::parse_alert_webhook_flag(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::msg)?;
    let filters: Vec<(String, String, String)> = args
        .alert_filter
        .iter()
        .map(|s| alert_config::parse_alert_filter_flag(s))
        .collect::<Result<Vec<_>, _>>()
        .map_err(anyhow::Error::msg)?;
    config = alert_config::apply_cli_overrides(config, &webhooks, &filters);
    let dispatcher = Arc::new(AlertDispatcher::new(config));

    if dispatcher.has_webhooks() {
        tracing::info!("webhook alert channels configured");
    }

    let ring = SnapshotRing::new();
    let ring_bg = ring.clone();
    let state_bg = state.clone();
    let db_bg = db.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db_bg, ring_bg).await;
    });

    // Brief warm-up so first tool call isn't stale
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    // When --dashboard is set, run the dashboard as the primary task.
    // The MCP stdio server only works when a client is connected via stdin,
    // so in dashboard-only mode we skip it and just serve the web UI.
    #[cfg(feature = "dashboard")]
    if args.dashboard {
        return axon_server::dashboard::run_dashboard(state, db, args.dashboard_port).await;
    }

    run_server(state, db, ring, dispatcher).await
}

async fn run_diagnose() -> Result<()> {
    tracing::info!("collecting system data (4s)");

    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let db_path = persistence::default_db_path()?;
    let db = persistence::open(db_path)?;

    // Check for recent alerts (last 60s) from persistent DB before collecting
    let recent_alerts = persistence::query_alerts(&db, 60, None, None, 5).unwrap_or_default();

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db, SnapshotRing::new()).await;
    });

    // Wait for at least 2 EWMA ticks (4s) so baselines stabilise
    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    let guard = state.lock().unwrap();
    let blame = &guard.blame;
    let hw = &guard.hw;

    // For diagnose (point-in-time snapshot), compute impact level directly from
    // the score, bypassing the persistence check. The persistence check exists
    // to avoid false positives in a long-running session, but diagnose only runs
    // for 4 seconds — if the score is high, the system IS under stress right now.
    let instant_level = axon_core::impact::score_to_level(blame.anomaly_score, u32::MAX);
    let impact_msg = axon_core::impact::impact_message(&instant_level, &blame.anomaly_type);

    println!();
    // Prefer group display when multiple processes are grouped
    if let Some(g) = &blame.culprit_group {
        if blame.anomaly_score > 0.10 && g.process_count > 1 {
            println!(
                "[warn] {} ({} processes)  --  {:.0}% CPU,  {:.1}GB RAM",
                g.name, g.process_count, g.total_cpu_pct, g.total_ram_gb
            );
            println!("       Impact: {}", impact_msg);
            println!("       Fix:    {}", blame.fix);
        } else if let Some(p) = &blame.culprit {
            if blame.anomaly_score > 0.10 {
                println!(
                    "[warn] {} (PID {})  --  {:.0}% CPU,  {:.1}GB RAM",
                    p.cmd, p.pid, p.cpu_pct, p.ram_gb
                );
                println!("       Impact: {}", impact_msg);
                println!("       Fix:    {}", blame.fix);
            }
        }
    } else if let Some(p) = &blame.culprit {
        if blame.anomaly_score > 0.10 {
            println!(
                "[warn] {} (PID {})  --  {:.0}% CPU,  {:.1}GB RAM",
                p.cmd, p.pid, p.cpu_pct, p.ram_gb
            );
            println!("       Impact: {}", impact_msg);
            println!("       Fix:    {}", blame.fix);
        }
    }

    // Show healthy state if no warning was printed
    let showed_warning =
        blame.anomaly_score > 0.10 && (blame.culprit.is_some() || blame.culprit_group.is_some());
    if !showed_warning {
        println!("[ok]  System is healthy. No significant anomalies detected.");
        println!(
            "      CPU: {:.0}%   RAM: {:.1}/{:.0}GB",
            hw.cpu_usage_pct, hw.ram_used_gb, hw.ram_total_gb
        );
    }

    if let Some(t) = hw.die_temp_celsius {
        let throttle = if hw.throttling { "  [THROTTLING]" } else { "" };
        println!("      Temp:   {:.0}C{}", t, throttle);
    }

    if let Some(b) = &guard.battery {
        println!("      Battery: {}", b.narrative);
    }

    // Show stale axon instance warnings
    if !blame.stale_axon_pids.is_empty() {
        let pids = blame
            .stale_axon_pids
            .iter()
            .map(|p| p.to_string())
            .collect::<Vec<_>>()
            .join(" ");
        #[cfg(target_os = "windows")]
        let kill_hint = format!("taskkill /F /PID {}", pids.replace(' ', " /PID "));
        #[cfg(not(target_os = "windows"))]
        let kill_hint = format!("kill {}", pids);
        println!(
            "[warn] {} stale axon instance(s) detected (PIDs: {}). Kill: {}",
            blame.stale_axon_pids.len(),
            pids,
            kill_hint
        );
    }

    // Show startup warnings (e.g. stale siblings detected at profile build time)
    for w in &guard.profile.startup_warnings {
        println!("[warn] {}", w);
    }

    // Show recent-recovery notice so agents don't pile on work immediately
    if !recent_alerts.is_empty() && !showed_warning {
        let critical_count = recent_alerts
            .iter()
            .filter(|a| a.severity == axon_core::types::AlertSeverity::Critical)
            .count();
        if critical_count > 0 {
            println!(
                "[info] Recently recovered: {} critical alert(s) in last 60s. Consider a brief cooldown.",
                critical_count
            );
        } else {
            println!(
                "[info] Recently recovered: {} alert(s) in last 60s.",
                recent_alerts.len()
            );
        }
    }

    println!();
    Ok(())
}

async fn run_status() -> Result<()> {
    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let db_path = persistence::default_db_path()?;
    let db = persistence::open(db_path)?;

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db, SnapshotRing::new()).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let guard = state.lock().unwrap();
    let json = serde_json::to_string_pretty(&guard.hw)?;
    println!("{}", json);
    Ok(())
}

async fn run_query(tool: &str) -> Result<()> {
    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let db_path = persistence::default_db_path()?;
    let db = persistence::open(db_path)?;

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db, SnapshotRing::new()).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    let json_str = {
        let guard = state.lock().unwrap();
        match tool {
            "process_blame" => {
                let narrative = axon_server::blame_narrative_pub(&guard.blame);
                let response = axon_core::types::McpResponse::success(guard.blame.clone(), narrative);
                serde_json::to_string_pretty(&response)?
            }
            "hw_snapshot" => {
                let narrative = axon_server::hw_narrative_pub(&guard.hw);
                let response = axon_core::types::McpResponse::success(guard.hw.clone(), narrative);
                serde_json::to_string_pretty(&response)?
            }
            "battery_status" => match &guard.battery {
                Some(b) => {
                    let narrative = b.narrative.clone();
                    let response = axon_core::types::McpResponse::success(b.clone(), narrative);
                    serde_json::to_string_pretty(&response)?
                }
                None => r#"{"ok":false,"narrative":"Battery information unavailable."}"#.to_string(),
            },
            "system_profile" => {
                let p = &guard.profile;
                let mut narrative = format!(
                    "{} ({}) — {} cores, {:.0}GB RAM, {}.",
                    p.model_id, p.chip, p.core_count, p.ram_total_gb, p.os_version
                );
                for w in &p.startup_warnings {
                    narrative.push_str(&format!(" [WARN] {}", w));
                }
                let response = axon_core::types::McpResponse::success(p.clone(), narrative);
                serde_json::to_string_pretty(&response)?
            }
            "session_health" => {
                drop(guard); // release collector state lock before DB query
                let since = chrono::Utc::now() - chrono::Duration::hours(1);
                let db_path = axon_core::persistence::default_db_path()?;
                let db = axon_core::persistence::open(db_path)?;
                let health = axon_core::persistence::query_session_health(&db, since)?;
                let narrative =
                    axon_server::session_health_narrative_pub(&health);
                let response = axon_core::types::McpResponse::success(health, narrative);
                serde_json::to_string_pretty(&response)?
            }
            "gpu_snapshot" => {
                let gpu = guard.gpu.clone();
                drop(guard);
                match gpu {
                    Some(g) => {
                        let narrative = axon_server::gpu_narrative_pub(&g);
                        let response = axon_core::types::McpResponse::success(g, narrative);
                        serde_json::to_string_pretty(&response)?
                    }
                    None => r#"{"ok":false,"narrative":"GPU metrics unavailable."}"#.to_string(),
                }
            }
            "hardware_trend" => {
                drop(guard); // release lock before DB query
                let db_path = axon_core::persistence::default_db_path()?;
                let db = axon_core::persistence::open(db_path)?;
                let range_secs = axon_core::persistence::parse_time_range("last_24h")
                    .expect("default range is valid");
                let bucket_secs = axon_core::persistence::parse_interval("15m")
                    .expect("default interval is valid");
                let trend = axon_core::persistence::query_trend(&db, range_secs, bucket_secs)?;
                let narrative = axon_server::trend_narrative_pub(&trend, "last_24h");
                let response = axon_core::types::McpResponse::success(trend, narrative);
                serde_json::to_string_pretty(&response)?
            }
            other => anyhow::bail!(
                "Unknown tool '{}'. Supported: process_blame, hw_snapshot, battery_status, system_profile, session_health, gpu_snapshot, hardware_trend",
                other
            ),
        }
    };

    println!("{}", json_str);
    Ok(())
}

fn run_setup(target: Option<&str>) -> Result<()> {
    match target {
        Some("claude-desktop") => setup_claude_desktop(),
        Some("claude-code") | Some("claude-cli") => setup_claude_code(),
        Some("cursor") => setup_cursor(),
        Some("vscode") | Some("vs-code") => setup_vscode(),
        Some(other) => anyhow::bail!(
            "Unknown target '{}'. Supported: claude-desktop, claude-code, cursor, vscode",
            other
        ),
        None => setup_all(),
    }
}

// ── Shared Helpers ───────────────────────────────────────────────────────────

fn bin_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("axon"))
}

/// Platform-aware config path for Claude Desktop.
fn claude_desktop_config_path(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Claude/claude_desktop_config.json")
    }
    #[cfg(target_os = "linux")]
    {
        home.join(".config/Claude/claude_desktop_config.json")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        // Windows: %APPDATA%\Claude
        home.join("AppData/Roaming/Claude/claude_desktop_config.json")
    }
}

/// Platform-aware config path for VS Code.
fn vscode_config_path(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/Code/User/settings.json")
    }
    #[cfg(target_os = "linux")]
    {
        home.join(".config/Code/User/settings.json")
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        home.join("AppData/Roaming/Code/User/settings.json")
    }
}

/// Platform-aware data directory for axon.
fn axon_data_dir(home: &std::path::Path) -> std::path::PathBuf {
    #[cfg(target_os = "macos")]
    {
        home.join("Library/Application Support/axon")
    }
    #[cfg(not(target_os = "macos"))]
    {
        dirs::data_local_dir()
            .map(|d| d.join("axon"))
            .unwrap_or_else(|| home.join(".local/share/axon"))
    }
}

fn mcp_entry() -> serde_json::Value {
    serde_json::json!({
        "command": bin_path().to_string_lossy(),
        "args": ["serve"]
    })
}

/// Upsert axon into a JSON config file with `{ "mcpServers": { ... } }` format.
/// Returns true if the file was written (i.e. not already configured).
fn upsert_mcp_config(config_path: &std::path::Path) -> Result<bool> {
    if config_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(config_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&raw) {
                if config
                    .get("mcpServers")
                    .and_then(|s| s.get("axon"))
                    .is_some()
                {
                    return Ok(false);
                }
            }
        }
    }

    let mut config: serde_json::Value = if config_path.exists() {
        std::fs::read_to_string(config_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }
    config["mcpServers"]["axon"] = mcp_entry();

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(true)
}

/// Upsert axon into VS Code settings.json which uses `{ "mcp": { "servers": { ... } } }`.
/// Returns true if the file was written.
fn upsert_vscode_config(config_path: &std::path::Path) -> Result<bool> {
    if config_path.exists() {
        if let Ok(raw) = std::fs::read_to_string(config_path) {
            if let Ok(config) = serde_json::from_str::<serde_json::Value>(&raw) {
                if config
                    .get("mcp")
                    .and_then(|m| m.get("servers"))
                    .and_then(|s| s.get("axon"))
                    .is_some()
                {
                    return Ok(false);
                }
            }
        }
    }

    let mut config: serde_json::Value = if config_path.exists() {
        std::fs::read_to_string(config_path)
            .ok()
            .and_then(|raw| serde_json::from_str(&raw).ok())
            .unwrap_or_else(|| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    if config.get("mcp").is_none() {
        config["mcp"] = serde_json::json!({});
    }
    if config["mcp"].get("servers").is_none() {
        config["mcp"]["servers"] = serde_json::json!({});
    }
    config["mcp"]["servers"]["axon"] = serde_json::json!({
        "type": "stdio",
        "command": bin_path().to_string_lossy(),
        "args": ["serve"]
    });

    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(config_path, serde_json::to_string_pretty(&config)?)?;
    Ok(true)
}

// ── Setup All ────────────────────────────────────────────────────────────────

fn setup_all() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let mut configured = 0;
    let mut skipped = 0;

    // Claude Desktop — configure if the app directory exists
    let claude_config = claude_desktop_config_path(&home);
    if claude_config.parent().map(|p| p.exists()).unwrap_or(false) || claude_config.exists() {
        match upsert_mcp_config(&claude_config) {
            Ok(true) => {
                println!(
                    "[ok] Configured Claude Desktop at {}",
                    claude_config.display()
                );
                println!("     Restart Claude Desktop to apply changes.");
                configured += 1;
            }
            Ok(false) => {
                println!("[ok] Claude Desktop already configured.");
                skipped += 1;
            }
            Err(e) => println!("[err] Claude Desktop: {}", e),
        }
    }

    // Cursor — configure if ~/.cursor exists
    let cursor_config = home.join(".cursor/mcp.json");
    if cursor_config.parent().map(|p| p.exists()).unwrap_or(false) || cursor_config.exists() {
        match upsert_mcp_config(&cursor_config) {
            Ok(true) => {
                println!("[ok] Configured Cursor at {}", cursor_config.display());
                println!("     Restart Cursor to apply changes.");
                configured += 1;
            }
            Ok(false) => {
                println!("[ok] Cursor already configured.");
                skipped += 1;
            }
            Err(e) => println!("[err] Cursor: {}", e),
        }
    }

    // VS Code — only if settings.json already exists
    let vscode_config = vscode_config_path(&home);
    if vscode_config.exists() {
        match upsert_vscode_config(&vscode_config) {
            Ok(true) => {
                println!("[ok] Configured VS Code at {}", vscode_config.display());
                println!("     Restart VS Code to apply changes.");
                configured += 1;
            }
            Ok(false) => {
                println!("[ok] VS Code already configured.");
                skipped += 1;
            }
            Err(e) => println!("[err] VS Code: {}", e),
        }
    }

    if configured == 0 && skipped == 0 {
        println!("[info] No supported agents detected (Claude Desktop, Cursor, VS Code).");
        println!("       Run 'axon setup <target>' to configure a specific agent.");
    }

    Ok(())
}

// ── Setup Helpers ─────────────────────────────────────────────────────────────

fn setup_claude_desktop() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let config_path = claude_desktop_config_path(&home);

    let wrote = upsert_mcp_config(&config_path)?;
    if wrote {
        println!("[ok] Updated {}", config_path.display());
        println!("     Restart Claude Desktop to apply changes.");
    } else {
        println!("[ok] Already configured at {}", config_path.display());
    }
    Ok(())
}

fn setup_cursor() -> Result<()> {
    let config_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".cursor/mcp.json");

    let wrote = upsert_mcp_config(&config_path)?;
    if wrote {
        println!("[ok] Updated {}", config_path.display());
        println!("     Restart Cursor to apply changes.");
    } else {
        println!("[ok] Already configured at {}", config_path.display());
    }
    Ok(())
}

fn setup_vscode() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let config_path = vscode_config_path(&home);

    let wrote = upsert_vscode_config(&config_path)?;
    if wrote {
        println!("[ok] Updated {}", config_path.display());
        println!("     Restart VS Code to apply changes.");
    } else {
        println!("[ok] Already configured at {}", config_path.display());
    }
    Ok(())
}

fn setup_claude_code() -> Result<()> {
    use std::process::Command;

    let full_path = bin_path();
    let status = Command::new("claude")
        .args([
            "mcp",
            "add",
            "axon",
            "--",
            &full_path.to_string_lossy(),
            "serve",
        ])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("[ok] axon added to Claude Code.");
            println!("     Verify with: claude mcp list");
        }
        Ok(_) => {
            anyhow::bail!("claude mcp add failed. Check that Claude Code is installed.");
        }
        Err(_) => {
            eprintln!("'claude' command not found. Printing config manually:\n");
            println!("Add this to your Claude Code MCP config:");
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "axon": mcp_entry()
                }))?
            );
        }
    }
    Ok(())
}

// ── Setup List ───────────────────────────────────────────────────────────────

fn run_setup_list() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;

    struct AgentInfo {
        name: &'static str,
        configured: bool,
        config_path: String,
        binary_path: String,
        detected: bool,
    }

    let mut agents: Vec<AgentInfo> = Vec::new();

    // Claude Desktop
    let claude_config = claude_desktop_config_path(&home);
    let claude_ok = check_mcp_config(&claude_config, "mcpServers");
    let claude_bin = get_configured_binary(&claude_config, &["mcpServers", "axon", "command"]);
    agents.push(AgentInfo {
        name: "Claude Desktop",
        configured: claude_ok,
        config_path: claude_config.display().to_string(),
        binary_path: claude_bin,
        detected: claude_config.parent().map(|p| p.exists()).unwrap_or(false),
    });

    // Cursor
    let cursor_config = home.join(".cursor/mcp.json");
    let cursor_ok = check_mcp_config(&cursor_config, "mcpServers");
    let cursor_bin = get_configured_binary(&cursor_config, &["mcpServers", "axon", "command"]);
    agents.push(AgentInfo {
        name: "Cursor",
        configured: cursor_ok,
        config_path: cursor_config.display().to_string(),
        binary_path: cursor_bin,
        detected: cursor_config.parent().map(|p| p.exists()).unwrap_or(false),
    });

    // VS Code
    let vscode_config = vscode_config_path(&home);
    let vscode_ok = check_vscode_config(&vscode_config);
    let vscode_bin = get_configured_binary(&vscode_config, &["mcp", "servers", "axon", "command"]);
    agents.push(AgentInfo {
        name: "VS Code",
        configured: vscode_ok,
        config_path: vscode_config.display().to_string(),
        binary_path: vscode_bin,
        detected: vscode_config.exists(),
    });

    // Claude Code
    let claude_code_ok = check_claude_code();
    agents.push(AgentInfo {
        name: "Claude Code",
        configured: claude_code_ok,
        config_path: "(managed by claude CLI)".to_string(),
        binary_path: if claude_code_ok {
            "via claude mcp".to_string()
        } else {
            "-".to_string()
        },
        detected: which_exists("claude"),
    });

    // Data directories
    let config_dir = home.join(".config/axon");
    let data_dir = axon_data_dir(&home);

    // Print structured output
    println!("Agent            Status        Config");
    println!("---------------  ----------    ------");
    let mut configured_count = 0;
    for a in &agents {
        let status = if a.configured {
            configured_count += 1;
            "[ok]"
        } else if a.detected {
            "[--]"
        } else {
            "[??]"
        };
        println!("{:<16} {:<12}  {}", a.name, status, a.config_path);
        if a.configured {
            println!("{:<16} {:<12}  binary: {}", "", "", a.binary_path);
        }
    }

    println!();
    println!(
        "Data:   {:<10} {}",
        if data_dir.exists() { "[ok]" } else { "[--]" },
        data_dir.display()
    );
    println!(
        "Config: {:<10} {}",
        if config_dir.exists() { "[ok]" } else { "[--]" },
        config_dir.display()
    );
    println!();
    println!(
        "{} of {} agent(s) configured.",
        configured_count,
        agents.iter().filter(|a| a.detected).count()
    );
    Ok(())
}

/// Walk a JSON path and return the string value at the end, or "-".
fn get_configured_binary(path: &std::path::Path, keys: &[&str]) -> String {
    if !path.exists() {
        return "-".to_string();
    }
    let raw = match std::fs::read_to_string(path) {
        Ok(r) => r,
        Err(_) => return "-".to_string(),
    };
    let config: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(c) => c,
        Err(_) => return "-".to_string(),
    };
    let mut current = &config;
    for key in keys {
        match current.get(key) {
            Some(v) => current = v,
            None => return "-".to_string(),
        }
    }
    current.as_str().unwrap_or("-").to_string()
}

fn which_exists(cmd: &str) -> bool {
    use std::process::Command;
    Command::new("which")
        .arg(cmd)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn check_mcp_config(path: &std::path::Path, key: &str) -> bool {
    if !path.exists() {
        return false;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|cfg| cfg.get(key)?.get("axon").cloned())
        .is_some()
}

fn check_vscode_config(path: &std::path::Path) -> bool {
    if !path.exists() {
        return false;
    }
    std::fs::read_to_string(path)
        .ok()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(&raw).ok())
        .and_then(|cfg| cfg.get("mcp")?.get("servers")?.get("axon").cloned())
        .is_some()
}

fn check_claude_code() -> bool {
    use std::process::Command;
    Command::new("claude")
        .args(["mcp", "get", "axon"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

// ── Uninstall ────────────────────────────────────────────────────────────────

fn run_uninstall(target: Option<&str>) -> Result<()> {
    match target {
        Some("claude-desktop") => uninstall_claude_desktop()?,
        Some("claude-code") | Some("claude-cli") => uninstall_claude_code()?,
        Some("cursor") => uninstall_cursor()?,
        Some("vscode") | Some("vs-code") => uninstall_vscode()?,
        Some(other) => anyhow::bail!(
            "Unknown target '{}'. Supported: claude-desktop, claude-code, cursor, vscode",
            other
        ),
        None => uninstall_all()?,
    }

    purge_data()?;
    Ok(())
}

fn uninstall_all() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let mut removed = 0;

    // Claude Desktop
    let claude_config = claude_desktop_config_path(&home);
    if remove_from_mcp_config(&claude_config, "mcpServers")? {
        println!("[ok] Removed axon from Claude Desktop config.");
        removed += 1;
    }

    // Cursor
    let cursor_config = home.join(".cursor/mcp.json");
    if remove_from_mcp_config(&cursor_config, "mcpServers")? {
        println!("[ok] Removed axon from Cursor config.");
        removed += 1;
    }

    // VS Code
    let vscode_config = vscode_config_path(&home);
    if remove_from_vscode_config(&vscode_config)? {
        println!("[ok] Removed axon from VS Code config.");
        removed += 1;
    }

    // Claude Code
    if uninstall_claude_code_inner() {
        println!("[ok] Removed axon from Claude Code.");
        removed += 1;
    }

    if removed == 0 {
        println!("[info] axon was not configured in any agent.");
    } else {
        println!(
            "[info] Removed from {} agent(s). Restart agents to apply.",
            removed
        );
    }

    Ok(())
}

/// Remove the "axon" key from a `{ "<key>": { "axon": ... } }` config file.
/// Returns true if axon was found and removed.
fn remove_from_mcp_config(path: &std::path::Path, key: &str) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)?;
    let mut config: serde_json::Value = serde_json::from_str(&raw)?;

    let had_axon = config.get(key).and_then(|s| s.get("axon")).is_some();

    if !had_axon {
        return Ok(false);
    }

    if let Some(servers) = config.get_mut(key).and_then(|s| s.as_object_mut()) {
        servers.remove("axon");
    }

    std::fs::write(path, serde_json::to_string_pretty(&config)?)?;
    Ok(true)
}

/// Remove axon from VS Code's `{ "mcp": { "servers": { "axon": ... } } }` structure.
fn remove_from_vscode_config(path: &std::path::Path) -> Result<bool> {
    if !path.exists() {
        return Ok(false);
    }
    let raw = std::fs::read_to_string(path)?;
    let mut config: serde_json::Value = serde_json::from_str(&raw)?;

    let had_axon = config
        .get("mcp")
        .and_then(|m| m.get("servers"))
        .and_then(|s| s.get("axon"))
        .is_some();

    if !had_axon {
        return Ok(false);
    }

    if let Some(servers) = config
        .get_mut("mcp")
        .and_then(|m| m.get_mut("servers"))
        .and_then(|s| s.as_object_mut())
    {
        servers.remove("axon");
    }

    std::fs::write(path, serde_json::to_string_pretty(&config)?)?;
    Ok(true)
}

fn uninstall_claude_desktop() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let path = claude_desktop_config_path(&home);
    if remove_from_mcp_config(&path, "mcpServers")? {
        println!("[ok] Removed axon from Claude Desktop config.");
        println!("     Restart Claude Desktop to apply changes.");
    } else {
        println!("[info] axon was not configured in Claude Desktop.");
    }
    Ok(())
}

fn uninstall_cursor() -> Result<()> {
    let path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join(".cursor/mcp.json");
    if remove_from_mcp_config(&path, "mcpServers")? {
        println!("[ok] Removed axon from Cursor config.");
        println!("     Restart Cursor to apply changes.");
    } else {
        println!("[info] axon was not configured in Cursor.");
    }
    Ok(())
}

fn uninstall_vscode() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;
    let path = vscode_config_path(&home);
    if remove_from_vscode_config(&path)? {
        println!("[ok] Removed axon from VS Code config.");
        println!("     Restart VS Code to apply changes.");
    } else {
        println!("[info] axon was not configured in VS Code.");
    }
    Ok(())
}

fn uninstall_claude_code() -> Result<()> {
    if uninstall_claude_code_inner() {
        println!("[ok] Removed axon from Claude Code.");
    } else {
        println!("[info] axon was not configured in Claude Code (or 'claude' CLI not found).");
    }
    Ok(())
}

fn uninstall_claude_code_inner() -> bool {
    use std::process::Command;
    Command::new("claude")
        .args(["mcp", "remove", "axon"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn purge_data() -> Result<()> {
    let home = dirs::home_dir().ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?;

    // Platform-aware config directory
    #[cfg(target_os = "windows")]
    let config_dir = home.join("AppData/Roaming/axon");
    #[cfg(not(target_os = "windows"))]
    let config_dir = home.join(".config/axon");
    if config_dir.exists() {
        std::fs::remove_dir_all(&config_dir)?;
        println!("[ok] Removed {}", config_dir.display());
    }

    let data_dir = axon_data_dir(&home);
    if data_dir.exists() {
        std::fs::remove_dir_all(&data_dir)?;
        println!("[ok] Removed {}", data_dir.display());
    }

    println!("[ok] Purged all axon data and config.");
    Ok(())
}
