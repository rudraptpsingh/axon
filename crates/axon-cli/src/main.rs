use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Args, Parser, Subcommand};
use std::path::PathBuf;
use tracing_subscriber::EnvFilter;

use axon_core::alert_config;
use axon_core::alert_dispatch::AlertDispatcher;
use axon_core::collector::{build_system_profile, start_collector, AppState};
use axon_core::persistence;
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

#[derive(Args, Debug, Clone)]
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
}

impl Default for ServeArgs {
    fn default() -> Self {
        Self {
            config_dir: None,
            alert_webhook: vec![],
            alert_filter: vec![],
        }
    }
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
        /// Tool name: process_blame | hw_snapshot | battery_status | system_profile
        #[arg(value_name = "TOOL")]
        tool: String,
    },

    /// Auto-configure an AI agent to use axon
    Setup {
        /// Target client: claude-desktop | claude-code | cursor | vscode
        #[arg(value_name = "TARGET")]
        target: String,
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

    // Auto-setup all supported agents on first run
    auto_setup_all();

    match cli.command {
        None => run_serve(ServeArgs::default()).await,
        Some(Commands::Serve(args)) => run_serve(args).await,
        Some(Commands::Diagnose) => run_diagnose().await,
        Some(Commands::Status) => run_status().await,
        Some(Commands::Query { tool }) => run_query(&tool).await,
        Some(Commands::Setup { target }) => run_setup(&target),
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

    let state_bg = state.clone();
    let db_bg = db.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db_bg).await;
    });

    // Brief warm-up so first tool call isn't stale
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    run_server(state, db, dispatcher).await
}

async fn run_diagnose() -> Result<()> {
    tracing::info!("collecting system data (4s)");

    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let db_path = persistence::default_db_path()?;
    let db = persistence::open(db_path)?;

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg, db).await;
    });

    // Wait for at least 2 EWMA ticks (4s) so baselines stabilise
    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    let guard = state.lock().unwrap();
    let blame = &guard.blame;
    let hw = &guard.hw;

    println!();
    // Prefer group display when multiple processes are grouped
    if let Some(g) = &blame.culprit_group {
        if blame.anomaly_score > 0.10 && g.process_count > 1 {
            println!(
                "[warn] {} ({} processes)  --  {:.0}% CPU,  {:.1}GB RAM",
                g.name, g.process_count, g.total_cpu_pct, g.total_ram_gb
            );
            println!("       Impact: {}", blame.impact);
            println!("       Fix:    {}", blame.fix);
        } else if let Some(p) = &blame.culprit {
            if blame.anomaly_score > 0.10 {
                println!(
                    "[warn] {} (PID {})  --  {:.0}% CPU,  {:.1}GB RAM",
                    p.cmd, p.pid, p.cpu_pct, p.ram_gb
                );
                println!("       Impact: {}", blame.impact);
                println!("       Fix:    {}", blame.fix);
            }
        }
    } else if let Some(p) = &blame.culprit {
        if blame.anomaly_score > 0.10 {
            println!(
                "[warn] {} (PID {})  --  {:.0}% CPU,  {:.1}GB RAM",
                p.cmd, p.pid, p.cpu_pct, p.ram_gb
            );
            println!("       Impact: {}", blame.impact);
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
        start_collector(state_bg, db).await;
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
        start_collector(state_bg, db).await;
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
                let narrative = format!(
                    "{} ({}) — {} cores, {:.0}GB RAM, {}.",
                    p.model_id, p.chip, p.core_count, p.ram_total_gb, p.os_version
                );
                let response = axon_core::types::McpResponse::success(p.clone(), narrative);
                serde_json::to_string_pretty(&response)?
            }
            other => anyhow::bail!(
                "Unknown tool '{}'. Supported: process_blame, hw_snapshot, battery_status, system_profile",
                other
            ),
        }
    };

    println!("{}", json_str);
    Ok(())
}

fn run_setup(target: &str) -> Result<()> {
    match target {
        "claude-desktop" => setup_claude_desktop(),
        "claude-code" | "claude-cli" => setup_claude_code(),
        "cursor" => setup_cursor(),
        "vscode" | "vs-code" => setup_vscode(),
        other => anyhow::bail!(
            "Unknown target '{}'. Supported: claude-desktop, claude-code, cursor, vscode",
            other
        ),
    }
}

// ── Shared Helpers ───────────────────────────────────────────────────────────

fn bin_path() -> std::path::PathBuf {
    std::env::current_exe().unwrap_or_else(|_| std::path::PathBuf::from("axon"))
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

// ── Auto-Setup ───────────────────────────────────────────────────────────────

fn auto_setup_all() {
    let Some(home) = dirs::home_dir() else {
        return;
    };

    // Claude Desktop
    let claude_config = home.join("Library/Application Support/Claude/claude_desktop_config.json");
    match upsert_mcp_config(&claude_config) {
        Ok(true) => eprintln!(
            "Auto-configured Claude Desktop at {}",
            claude_config.display()
        ),
        Ok(false) => {}
        Err(e) => eprintln!("Warning: auto-setup of Claude Desktop failed: {}", e),
    }

    // Cursor (global config)
    let cursor_config = home.join(".cursor/mcp.json");
    match upsert_mcp_config(&cursor_config) {
        Ok(true) => eprintln!("Auto-configured Cursor at {}", cursor_config.display()),
        Ok(false) => {}
        Err(e) => eprintln!("Warning: auto-setup of Cursor failed: {}", e),
    }

    // VS Code (user settings)
    let vscode_config = home.join("Library/Application Support/Code/User/settings.json");
    if vscode_config.exists() {
        match upsert_vscode_config(&vscode_config) {
            Ok(true) => eprintln!("Auto-configured VS Code at {}", vscode_config.display()),
            Ok(false) => {}
            Err(e) => eprintln!("Warning: auto-setup of VS Code failed: {}", e),
        }
    }
}

// ── Setup Helpers ─────────────────────────────────────────────────────────────

fn setup_claude_desktop() -> Result<()> {
    let config_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join("Library/Application Support/Claude/claude_desktop_config.json");

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
    let config_path = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join("Library/Application Support/Code/User/settings.json");

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
