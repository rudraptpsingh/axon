use std::sync::{Arc, Mutex};

use anyhow::Result;
use clap::{Parser, Subcommand};
use tracing_subscriber::EnvFilter;

use mcp_station_core::collector::{build_system_profile, start_collector, AppState};
use mcp_station_server::run_server;

// ── CLI Definition ────────────────────────────────────────────────────────────

#[derive(Parser)]
#[command(name = "mcp-station")]
#[command(version)]
#[command(
    about = "Local hardware intelligence for AI-powered developers",
    long_about = "mcp-station gives AI coding agents (Cursor, Claude Code, Windsurf) \
    real-time awareness of your Mac's hardware: CPU load, die temperature, memory pressure, \
    and which process is responsible — without sending a single byte off-device."
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Start the MCP stdio server (used in claude_desktop_config.json)
    Serve,

    /// One-shot diagnosis: collect 4 seconds of data, then print the culprit
    Diagnose,

    /// Print current hardware snapshot as JSON
    Status,

    /// Auto-configure Claude Desktop or Claude CLI to use mcp-station
    Setup {
        /// Target client: claude-desktop | claude-cli
        #[arg(value_name = "TARGET")]
        target: String,
    },
}

// ── Entry Point ───────────────────────────────────────────────────────────────

#[tokio::main]
async fn main() -> Result<()> {
    // Log to stderr only — stdout is reserved for MCP JSON-RPC
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn")),
        )
        .with_writer(std::io::stderr)
        .without_time()
        .init();

    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Serve) | None => run_serve().await,
        Some(Commands::Diagnose) => run_diagnose().await,
        Some(Commands::Status) => run_status().await,
        Some(Commands::Setup { target }) => run_setup(&target),
    }
}

// ── Command Handlers ──────────────────────────────────────────────────────────

async fn run_serve() -> Result<()> {
    tracing::info!("mcp-station starting (stdio transport)");
    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    // Start collector background task
    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg).await;
    });

    // Brief warm-up so first tool call isn't stale
    tokio::time::sleep(tokio::time::Duration::from_millis(300)).await;

    run_server(state).await
}

async fn run_diagnose() -> Result<()> {
    eprintln!("🔍  Collecting system data (4s)...\n");

    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg).await;
    });

    // Wait for at least 2 EWMA ticks (4s) so baselines stabilise
    tokio::time::sleep(tokio::time::Duration::from_secs(4)).await;

    let guard = state.lock().unwrap();
    let blame = &guard.blame;
    let hw = &guard.hw;

    println!();
    match &blame.culprit {
        Some(p) if blame.anomaly_score > 0.10 => {
            println!(
                "⚠️   {} (PID {})  —  {:.0}% CPU,  {:.1}GB RAM",
                p.cmd, p.pid, p.cpu_pct, p.ram_gb
            );
            println!("     Impact: {}", blame.impact);
            println!("     Fix:    {}", blame.fix);
        }
        _ => {
            println!("✅   System is healthy. No significant anomalies detected.");
            println!(
                "     CPU: {:.0}%   RAM: {:.1}/{:.0}GB",
                hw.cpu_usage_pct, hw.ram_used_gb, hw.ram_total_gb
            );
        }
    }

    if let Some(t) = hw.die_temp_celsius {
        let throttle = if hw.throttling { "  ⚠️ throttling" } else { "" };
        println!("     Temp:   {:.0}°C{}", t, throttle);
    }

    if let Some(b) = &guard.battery {
        println!("     Battery: {}", b.narrative);
    }

    println!();
    Ok(())
}

async fn run_status() -> Result<()> {
    let profile = build_system_profile();
    let state = Arc::new(Mutex::new(AppState::new(profile)));

    let state_bg = state.clone();
    tokio::spawn(async move {
        start_collector(state_bg).await;
    });

    tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;

    let guard = state.lock().unwrap();
    let json = serde_json::to_string_pretty(&guard.hw)?;
    println!("{}", json);
    Ok(())
}

fn run_setup(target: &str) -> Result<()> {
    match target {
        "claude-desktop" => setup_claude_desktop(),
        "claude-cli" => setup_claude_cli(),
        other => anyhow::bail!(
            "Unknown target '{}'. Use 'claude-desktop' or 'claude-cli'.",
            other
        ),
    }
}

// ── Setup Helpers ─────────────────────────────────────────────────────────────

fn setup_claude_desktop() -> Result<()> {
    let config_dir = dirs::home_dir()
        .ok_or_else(|| anyhow::anyhow!("Cannot find home directory"))?
        .join("Library/Application Support/Claude");

    let config_path = config_dir.join("claude_desktop_config.json");

    let mcp_entry = serde_json::json!({
        "command": "mcp-station",
        "args": ["serve"]
    });

    // Read existing config or start fresh
    let mut config: serde_json::Value = if config_path.exists() {
        let raw = std::fs::read_to_string(&config_path)?;
        serde_json::from_str(&raw).unwrap_or_else(|_| serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Ensure mcpServers key exists
    if config.get("mcpServers").is_none() {
        config["mcpServers"] = serde_json::json!({});
    }
    config["mcpServers"]["mcp-station"] = mcp_entry;

    std::fs::create_dir_all(&config_dir)?;
    std::fs::write(&config_path, serde_json::to_string_pretty(&config)?)?;

    println!("✅  Updated {}", config_path.display());
    println!("    Restart Claude Desktop to apply changes.");
    println!();
    println!("    To verify: open Claude Desktop and run:");
    println!("    \"Call mcp-station's system_profile tool\"");
    Ok(())
}

fn setup_claude_cli() -> Result<()> {
    use std::process::Command;

    // claude mcp add mcp-station -- mcp-station serve
    let status = Command::new("claude")
        .args(["mcp", "add", "mcp-station", "--", "mcp-station", "serve"])
        .status();

    match status {
        Ok(s) if s.success() => {
            println!("✅  mcp-station added to Claude CLI.");
            println!("    Verify with: claude mcp list");
        }
        Ok(_) => {
            anyhow::bail!(
                "claude mcp add failed. Check that 'mcp-station' is in your PATH \
                (run: which mcp-station) and that Claude CLI is installed."
            );
        }
        Err(_) => {
            eprintln!("'claude' command not found. Printing config manually:\n");
            println!("Add this to your Claude CLI MCP config:");
            println!(
                "{}",
                serde_json::to_string_pretty(&serde_json::json!({
                    "mcp-station": {
                        "command": "mcp-station",
                        "args": ["serve"]
                    }
                }))?
            );
        }
    }
    Ok(())
}
