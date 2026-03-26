//! HTTP dashboard for visualizing Axon's agent network.
//!
//! Serves a single-page animated dashboard at `http://localhost:7670`
//! showing real-time hardware metrics, registered agents, alert history,
//! and agent-axon interaction traces with token usage.

use std::net::SocketAddr;
use std::sync::Arc;

use axon_core::{
    collector::SharedState,
    persistence::{self, DbHandle},
};
use axum::{
    extract::State,
    http::{header, StatusCode},
    response::{IntoResponse, Json},
    routing::{get, post},
    Router,
};
use serde::{Deserialize, Serialize};
use tokio::sync::RwLock;

// ── Shared Dashboard State ───────────────────────────────────────────────────

#[derive(Clone)]
pub struct DashboardState {
    pub hw_state: SharedState,
    pub db: DbHandle,
    pub interactions: Arc<RwLock<Vec<Interaction>>>,
    pub spawned_agents: Arc<RwLock<Vec<SpawnedAgent>>>,
    pub bench: Arc<RwLock<(String, BenchRun, BenchRun)>>, // (phase, naive, axon_aware)
}

// ── Interaction & Agent Types ────────────────────────────────────────────────

#[derive(Clone, Serialize)]
pub struct Interaction {
    pub ts: String,
    pub agent_id: String,
    pub agent_name: String,
    pub direction: String, // "agent_to_axon", "axon_to_agent", "agent_decision"
    pub tool: Option<String>,
    pub params: Option<String>,
    pub response: Option<String>,
    pub message: Option<String>,
    pub input_tokens: Option<u64>,
    pub output_tokens: Option<u64>,
}

#[derive(Clone, Serialize)]
pub struct SpawnedAgent {
    pub id: String,
    pub name: String,
    pub task: String,
    pub status: String, // "running", "completed", "failed"
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub pid: Option<u32>,
}

// ── API Response Types ───────────────────────────────────────────────────────

#[derive(Serialize)]
struct SnapshotResponse {
    cpu_pct: f64,
    ram_used_gb: f64,
    ram_total_gb: f64,
    ram_pressure: String,
    die_temp_c: Option<f64>,
    throttling: bool,
    disk_used_gb: f64,
    disk_total_gb: f64,
    disk_pressure: String,
    gpu_util_pct: Option<f64>,
    gpu_vram_used_mb: Option<f64>,
    gpu_vram_alloc_mb: Option<f64>,
    headroom: String,
    headroom_reason: String,
    impact_level: String,
    top_culprit: String,
    one_liner: String,
}

#[derive(Serialize)]
struct ProfileResponse {
    model_id: String,
    chip: String,
    core_count: usize,
    ram_total_gb: f64,
    os_version: String,
    axon_version: String,
}

#[derive(Serialize)]
struct AlertResponse {
    severity: String,
    alert_type: String,
    message: String,
    ts: String,
}

#[derive(Serialize)]
struct AgentInfo {
    name: String,
    registered: bool,
    config_path: String,
}

#[derive(Deserialize)]
struct SpawnRequest {
    agents: Option<Vec<AgentTask>>,
}

#[derive(Deserialize, Clone)]
struct AgentTask {
    name: String,
    prompt: String,
}

#[derive(Serialize)]
struct SpawnResponse {
    spawned: usize,
    agent_ids: Vec<String>,
}

// ── Default Agent Tasks ──────────────────────────────────────────────────────

fn default_agent_tasks() -> Vec<AgentTask> {
    vec![
        AgentTask {
            name: "System Checker".into(),
            prompt: "You have access to axon MCP tools. Call hw_snapshot to check current system state. \
                     Based on the headroom field, decide whether it's safe to start a heavy build. \
                     If headroom is insufficient, explain what you'd defer. \
                     Then call process_blame to identify what's slowing the system. \
                     Summarize your findings and decisions in 2-3 sentences.".into(),
        },
        AgentTask {
            name: "Resource Monitor".into(),
            prompt: "You have access to axon MCP tools. Call hw_snapshot to get current metrics. \
                     Then call battery_status to check power state. \
                     Then call gpu_snapshot to check GPU availability. \
                     Based on all three, recommend whether to run ML inference locally or defer. \
                     State your decision clearly.".into(),
        },
    ]
}

// ── Route Handlers ───────────────────────────────────────────────────────────

async fn serve_index() -> impl IntoResponse {
    let html = include_str!("../../../dashboard/index.html");
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
}

async fn api_snapshot(State(state): State<DashboardState>) -> Json<SnapshotResponse> {
    let guard = state.hw_state.lock().unwrap();
    let hw = &guard.hw;
    let gpu = &guard.gpu;

    Json(SnapshotResponse {
        cpu_pct: hw.cpu_usage_pct,
        ram_used_gb: hw.ram_used_gb,
        ram_total_gb: hw.ram_total_gb,
        ram_pressure: format!("{:?}", hw.ram_pressure).to_lowercase(),
        die_temp_c: hw.die_temp_celsius,
        throttling: hw.throttling,
        disk_used_gb: hw.disk_used_gb,
        disk_total_gb: hw.disk_total_gb,
        disk_pressure: format!("{:?}", hw.disk_pressure).to_lowercase(),
        gpu_util_pct: gpu.as_ref().and_then(|g| g.utilization_pct),
        gpu_vram_used_mb: gpu
            .as_ref()
            .and_then(|g| g.vram_used_bytes)
            .map(|b| b as f64 / 1_048_576.0),
        gpu_vram_alloc_mb: gpu
            .as_ref()
            .and_then(|g| g.vram_alloc_bytes)
            .map(|b| b as f64 / 1_048_576.0),
        headroom: format!("{:?}", hw.headroom).to_lowercase(),
        headroom_reason: hw.headroom_reason.clone(),
        impact_level: format!("{:?}", hw.impact_level).to_lowercase(),
        top_culprit: hw.top_culprit.clone(),
        one_liner: hw.one_liner.clone(),
    })
}

async fn api_profile(State(state): State<DashboardState>) -> Json<ProfileResponse> {
    let guard = state.hw_state.lock().unwrap();
    let p = &guard.profile;
    Json(ProfileResponse {
        model_id: p.model_id.clone(),
        chip: p.chip.clone(),
        core_count: p.core_count,
        ram_total_gb: p.ram_total_gb,
        os_version: p.os_version.clone(),
        axon_version: p.axon_version.clone(),
    })
}

async fn api_alerts(State(state): State<DashboardState>) -> Json<Vec<AlertResponse>> {
    let alerts = persistence::query_alerts(&state.db, 3600, None, None, 50).unwrap_or_default();
    Json(
        alerts
            .into_iter()
            .map(|a| AlertResponse {
                severity: format!("{:?}", a.severity).to_lowercase(),
                alert_type: format!("{:?}", a.alert_type),
                message: a.message,
                ts: a.ts.to_rfc3339(),
            })
            .collect(),
    )
}

async fn api_agents() -> Json<Vec<AgentInfo>> {
    let agents = scan_registered_agents();
    Json(agents)
}

async fn api_interactions(State(state): State<DashboardState>) -> Json<Vec<Interaction>> {
    let interactions = state.interactions.read().await;
    Json(interactions.clone())
}

async fn api_agent_status(State(state): State<DashboardState>) -> Json<Vec<SpawnedAgent>> {
    let agents = state.spawned_agents.read().await;
    Json(agents.clone())
}

async fn api_spawn_agents(
    State(state): State<DashboardState>,
    Json(body): Json<SpawnRequest>,
) -> Json<SpawnResponse> {
    // Clear previous run
    {
        let mut agents = state.spawned_agents.write().await;
        agents.clear();
    }
    {
        let mut ints = state.interactions.write().await;
        ints.clear();
    }

    let tasks = body.agents.unwrap_or_else(default_agent_tasks);
    let mut agent_ids = Vec::new();

    for (i, task) in tasks.into_iter().enumerate() {
        let id = format!("agent-{}-{}", i, chrono::Utc::now().timestamp_millis());
        let agent = SpawnedAgent {
            id: id.clone(),
            name: task.name.clone(),
            task: task.prompt.clone(),
            status: "running".into(),
            input_tokens: 0,
            output_tokens: 0,
            pid: None,
        };

        {
            let mut agents = state.spawned_agents.write().await;
            agents.push(agent);
        }

        // Spawn the Claude CLI process in a background task
        let interactions = state.interactions.clone();
        let spawned_agents = state.spawned_agents.clone();
        let agent_id = id.clone();
        let agent_name = task.name.clone();
        let prompt = task.prompt.clone();

        tokio::spawn(async move {
            run_claude_agent(agent_id, agent_name, prompt, interactions, spawned_agents).await;
        });

        agent_ids.push(id);
    }

    Json(SpawnResponse {
        spawned: agent_ids.len(),
        agent_ids,
    })
}

// ── Claude CLI Agent Runner ──────────────────────────────────────────────────

async fn run_claude_agent(
    agent_id: String,
    agent_name: String,
    prompt: String,
    interactions: Arc<RwLock<Vec<Interaction>>>,
    spawned_agents: Arc<RwLock<Vec<SpawnedAgent>>>,
) {
    let now = || chrono::Utc::now().to_rfc3339();

    // Log: agent starting
    {
        let mut ints = interactions.write().await;
        ints.push(Interaction {
            ts: now(),
            agent_id: agent_id.clone(),
            agent_name: agent_name.clone(),
            direction: "agent_decision".into(),
            tool: None,
            params: None,
            response: None,
            message: Some(format!("Agent starting: {}", agent_name)),
            input_tokens: None,
            output_tokens: None,
        });
    }

    // Ensure MCP config file exists so spawned agents have axon tools
    let mcp_config_path = ensure_mcp_config().await;

    // Spawn claude CLI with MCP config and skip-permissions to avoid hanging
    let mut cmd_args = vec![
        "--print".to_string(),
        "--output-format".to_string(),
        "json".to_string(),
        "--model".to_string(),
        "claude-sonnet-4-20250514".to_string(),
        "--dangerously-skip-permissions".to_string(),
    ];
    if let Some(mcp_path) = &mcp_config_path {
        cmd_args.push("--mcp-config".to_string());
        cmd_args.push(mcp_path.clone());
    }
    cmd_args.push("-p".to_string());
    cmd_args.push(prompt.clone());

    let result = tokio::process::Command::new("claude")
        .args(&cmd_args)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();

    let child = match result {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("failed to spawn claude CLI: {}", e);
            let mut agents = spawned_agents.write().await;
            if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
                a.status = format!("failed: {}", e);
            }
            let mut ints = interactions.write().await;
            ints.push(Interaction {
                ts: now(),
                agent_id: agent_id.clone(),
                agent_name: agent_name.clone(),
                direction: "agent_decision".into(),
                tool: None,
                params: None,
                response: None,
                message: Some(format!("Failed to spawn: {}", e)),
                input_tokens: None,
                output_tokens: None,
            });
            return;
        }
    };

    // Record PID
    if let Some(pid) = child.id() {
        let mut agents = spawned_agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
            a.pid = Some(pid);
        }
    }

    // Wait for output
    let output = match child.wait_with_output().await {
        Ok(o) => o,
        Err(e) => {
            tracing::error!("claude CLI error: {}", e);
            let mut agents = spawned_agents.write().await;
            if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
                a.status = format!("failed: {}", e);
            }
            return;
        }
    };

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        tracing::warn!("claude CLI exited with {}: {}", output.status, stderr);
    }

    // Parse the JSON output to extract tool calls and token usage
    parse_claude_output(
        &agent_id,
        &agent_name,
        &stdout,
        &interactions,
        &spawned_agents,
    )
    .await;

    // Mark agent as completed
    {
        let mut agents = spawned_agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
            a.status = if output.status.success() {
                "completed".into()
            } else {
                "failed".into()
            };
        }
    }
}

async fn parse_claude_output(
    agent_id: &str,
    agent_name: &str,
    stdout: &str,
    interactions: &Arc<RwLock<Vec<Interaction>>>,
    spawned_agents: &Arc<RwLock<Vec<SpawnedAgent>>>,
) {
    let now = || chrono::Utc::now().to_rfc3339();

    // Try to parse each line as JSON (claude --output-format json outputs JSON lines)
    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let parsed: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                // Not JSON - might be plain text output. Log it as agent response.
                if line.len() > 5 {
                    let mut ints = interactions.write().await;
                    ints.push(Interaction {
                        ts: now(),
                        agent_id: agent_id.into(),
                        agent_name: agent_name.into(),
                        direction: "agent_decision".into(),
                        tool: None,
                        params: None,
                        response: None,
                        message: Some(truncate(line, 300)),
                        input_tokens: None,
                        output_tokens: None,
                    });
                }
                continue;
            }
        };

        // Extract token usage from the response
        if let Some(usage) = parsed.get("usage") {
            let input = usage
                .get("input_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);
            let output = usage
                .get("output_tokens")
                .and_then(|v| v.as_u64())
                .unwrap_or(0);

            let mut agents = spawned_agents.write().await;
            if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
                a.input_tokens += input;
                a.output_tokens += output;
            }
        }

        // Extract tool_use blocks (agent calling axon)
        if let Some(content) = parsed.get("content").and_then(|c| c.as_array()) {
            for block in content {
                let block_type = block.get("type").and_then(|t| t.as_str()).unwrap_or("");

                if block_type == "tool_use" {
                    let tool_name = block
                        .get("name")
                        .and_then(|n| n.as_str())
                        .unwrap_or("unknown");
                    let input_json = block
                        .get("input")
                        .map(|v| serde_json::to_string(v).unwrap_or_default())
                        .unwrap_or_default();

                    let mut ints = interactions.write().await;
                    ints.push(Interaction {
                        ts: now(),
                        agent_id: agent_id.into(),
                        agent_name: agent_name.into(),
                        direction: "agent_to_axon".into(),
                        tool: Some(tool_name.into()),
                        params: Some(truncate(&input_json, 200)),
                        response: None,
                        message: None,
                        input_tokens: None,
                        output_tokens: None,
                    });
                }

                if block_type == "tool_result" {
                    let tool_id = block
                        .get("tool_use_id")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");
                    let result_content = block
                        .get("content")
                        .map(|v| {
                            if let Some(s) = v.as_str() {
                                s.to_string()
                            } else {
                                serde_json::to_string(v).unwrap_or_default()
                            }
                        })
                        .unwrap_or_default();

                    let mut ints = interactions.write().await;
                    ints.push(Interaction {
                        ts: now(),
                        agent_id: agent_id.into(),
                        agent_name: agent_name.into(),
                        direction: "axon_to_agent".into(),
                        tool: Some(tool_id.into()),
                        params: None,
                        response: Some(truncate(&result_content, 300)),
                        message: None,
                        input_tokens: None,
                        output_tokens: None,
                    });
                }

                if block_type == "text" {
                    let text = block.get("text").and_then(|t| t.as_str()).unwrap_or("");
                    if !text.is_empty() {
                        let mut ints = interactions.write().await;
                        ints.push(Interaction {
                            ts: now(),
                            agent_id: agent_id.into(),
                            agent_name: agent_name.into(),
                            direction: "agent_decision".into(),
                            tool: None,
                            params: None,
                            response: None,
                            message: Some(truncate(text, 400)),
                            input_tokens: None,
                            output_tokens: None,
                        });
                    }
                }
            }
        }

        // Handle top-level result field (some claude output formats)
        if let Some(result) = parsed.get("result").and_then(|r| r.as_str()) {
            if !result.is_empty() {
                let mut ints = interactions.write().await;
                ints.push(Interaction {
                    ts: now(),
                    agent_id: agent_id.into(),
                    agent_name: agent_name.into(),
                    direction: "agent_decision".into(),
                    tool: None,
                    params: None,
                    response: None,
                    message: Some(truncate(result, 400)),
                    input_tokens: parsed
                        .get("usage")
                        .and_then(|u| u.get("input_tokens"))
                        .and_then(|v| v.as_u64()),
                    output_tokens: parsed
                        .get("usage")
                        .and_then(|u| u.get("output_tokens"))
                        .and_then(|v| v.as_u64()),
                });
            }
        }
    }
}

/// Ensure that an MCP config file exists for spawned agents.
/// Returns the path to the config file.
async fn ensure_mcp_config() -> Option<String> {
    let config_path = "/tmp/axon-dashboard-mcp.json";
    let path = std::path::Path::new(config_path);

    if path.exists() {
        if let Ok(content) = tokio::fs::read_to_string(path).await {
            if content.contains("axon") {
                return Some(config_path.to_string());
            }
        }
    }

    let axon_bin = which_axon();
    let config = serde_json::json!({
        "mcpServers": {
            "axon": {
                "command": axon_bin,
                "args": ["serve"]
            }
        }
    });

    match tokio::fs::write(path, serde_json::to_string_pretty(&config).unwrap()).await {
        Ok(_) => {
            tracing::info!("created MCP config at {}", config_path);
            Some(config_path.to_string())
        }
        Err(e) => {
            tracing::warn!("failed to write MCP config: {}", e);
            None
        }
    }
}

fn which_axon() -> String {
    // Try common locations
    let home = dirs::home_dir().unwrap_or_default();
    let candidates = [
        home.join(".cargo/bin/axon"),
        std::path::PathBuf::from("/usr/local/bin/axon"),
        std::path::PathBuf::from("/opt/homebrew/bin/axon"),
    ];
    for c in &candidates {
        if c.exists() {
            return c.to_string_lossy().to_string();
        }
    }
    "axon".to_string() // fall back to PATH
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        format!("{}...", &s[..max])
    }
}

// ── Agent Config Scanner ─────────────────────────────────────────────────────

fn scan_registered_agents() -> Vec<AgentInfo> {
    let mut agents = Vec::new();
    let home = dirs::home_dir().unwrap_or_default();

    #[cfg(target_os = "macos")]
    let claude_desktop_path =
        home.join("Library/Application Support/Claude/claude_desktop_config.json");
    #[cfg(target_os = "linux")]
    let claude_desktop_path = home.join(".config/Claude/claude_desktop_config.json");
    #[cfg(target_os = "windows")]
    let claude_desktop_path = home.join("AppData/Roaming/Claude/claude_desktop_config.json");

    agents.push(check_agent_config(
        "Claude Desktop",
        &claude_desktop_path.to_string_lossy(),
    ));

    let claude_code_path = home.join(".claude/settings.local.json");
    agents.push(check_agent_config(
        "Claude Code",
        &claude_code_path.to_string_lossy(),
    ));

    let cursor_path = home.join(".cursor/mcp.json");
    agents.push(check_agent_config("Cursor", &cursor_path.to_string_lossy()));

    let vscode_path = home.join(".vscode/mcp.json");
    agents.push(check_agent_config(
        "VS Code",
        &vscode_path.to_string_lossy(),
    ));

    agents
}

fn check_agent_config(name: &str, path: &str) -> AgentInfo {
    let registered = std::fs::read_to_string(path)
        .ok()
        .map(|content| content.contains("axon"))
        .unwrap_or(false);

    AgentInfo {
        name: name.to_string(),
        registered,
        config_path: path.to_string(),
    }
}

// ── Benchmark Types & Endpoints ──────────────────────────────────────────────

#[derive(Serialize, Clone, Default)]
pub struct BenchRun {
    pub label: String,
    pub batch_size: u32,
    pub total: u32,
    pub processed: u32,
    pub failed: u32,
    pub status: String,
    pub elapsed_s: f64,
    pub snapshots: Vec<BenchSnap>,
}

#[derive(Serialize, Clone)]
pub struct BenchSnap {
    pub t: f64,
    pub cpu: f64,
    pub ram_gb: f64,
    pub pressure: String,
}

#[derive(Serialize)]
struct BenchStatus {
    naive: BenchRun,
    axon_aware: BenchRun,
    phase: String,
}

async fn api_bench_start(State(state): State<DashboardState>) -> Json<serde_json::Value> {
    {
        let b = state.bench.read().await;
        if b.0 == "naive_running" || b.0 == "axon_running" || b.0 == "setup" {
            return Json(serde_json::json!({"error": "already running"}));
        }
    }
    let bench = state.bench.clone();
    let hw = state.hw_state.clone();
    tokio::spawn(async move {
        run_benchmark(bench, hw).await;
    });
    Json(serde_json::json!({"started": true}))
}

async fn api_bench_status(State(state): State<DashboardState>) -> Json<BenchStatus> {
    let b = state.bench.read().await;
    Json(BenchStatus {
        naive: b.1.clone(),
        axon_aware: b.2.clone(),
        phase: b.0.clone(),
    })
}

async fn run_benchmark(bench: Arc<RwLock<(String, BenchRun, BenchRun)>>, hw: SharedState) {
    let count: u32 = 30;
    let bench_path = find_bench_sh();
    tracing::info!(
        "benchmark starting with bench.sh at: {} (count={})",
        bench_path,
        count
    );

    // Setup
    {
        let mut b = bench.write().await;
        b.0 = "setup".into();
        b.1 = BenchRun {
            label: "Naive Agent (no Axon)".into(),
            batch_size: count,
            total: count,
            status: "pending".into(),
            ..Default::default()
        };
        b.2 = BenchRun {
            label: "Axon-Aware Agent".into(),
            total: count,
            status: "pending".into(),
            ..Default::default()
        };
    }
    tracing::info!("bench setup: bash {} setup 50 {}", bench_path, count);
    let setup = tokio::process::Command::new("bash")
        .args([&bench_path, "setup", "50", &count.to_string()])
        .output()
        .await;
    if let Ok(ref o) = setup {
        tracing::info!("setup done: {}", String::from_utf8_lossy(&o.stdout).trim());
    }

    // Naive run: all images at once
    {
        let mut b = bench.write().await;
        b.0 = "naive_running".into();
        b.1.status = "running".into();
    }

    let bench2 = bench.clone();
    let hw2 = hw.clone();
    let snap_task = tokio::spawn(collect_snapshots(bench2, hw2));

    tracing::info!("naive run: batch={} count={}", count, count);
    let t0 = std::time::Instant::now();
    let out = tokio::process::Command::new("bash")
        .args([
            &bench_path,
            "process",
            &count.to_string(),
            &count.to_string(),
        ])
        .output()
        .await;
    {
        let mut b = bench.write().await;
        b.1.elapsed_s = t0.elapsed().as_secs_f64();
        b.1.status = "done".into();
        if let Ok(ref o) = out {
            let stdout = String::from_utf8_lossy(&o.stdout);
            tracing::info!("naive result: {}", stdout.trim());
            // Parse last line as JSON (bench.sh may output progress + final JSON)
            if let Some(last_line) = stdout.lines().last() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(last_line) {
                    b.1.processed = v["processed"].as_u64().unwrap_or(0) as u32;
                    b.1.failed = v["failed"].as_u64().unwrap_or(0) as u32;
                    if let Some(e) = v["elapsed_s"].as_f64() {
                        b.1.elapsed_s = e;
                    }
                }
            }
        }
    }

    tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
    let _ = tokio::fs::remove_dir_all("/tmp/axon-bench-images/out").await;
    let _ = tokio::fs::create_dir_all("/tmp/axon-bench-images/out").await;

    // Axon-aware run: check hw and adapt batch size
    // Axon-aware decision: check hw_snapshot and pick conservative batch size
    let (batch_size, ram_pct, headroom) = {
        let g = hw.lock().unwrap();
        let pct = (g.hw.ram_used_gb / g.hw.ram_total_gb) * 100.0;
        let h = format!("{:?}", g.hw.headroom).to_lowercase();
        let batch = if pct > 75.0 {
            3
        } else if pct > 60.0 {
            5
        } else {
            8
        };
        (batch, pct, h)
    };
    tracing::info!(
        "axon decision: RAM {:.0}% headroom={} → batch_size={}",
        ram_pct,
        headroom,
        batch_size
    );
    {
        let mut b = bench.write().await;
        b.0 = "axon_running".into();
        b.2.status = "running".into();
        b.2.batch_size = batch_size;
    }

    tracing::info!("axon run: batch={} count={}", batch_size, count);
    let t1 = std::time::Instant::now();
    let out2 = tokio::process::Command::new("bash")
        .args([
            &bench_path,
            "process",
            &batch_size.to_string(),
            &count.to_string(),
        ])
        .output()
        .await;
    {
        let mut b = bench.write().await;
        b.2.elapsed_s = t1.elapsed().as_secs_f64();
        b.2.status = "done".into();
        if let Ok(ref o) = out2 {
            let stdout = String::from_utf8_lossy(&o.stdout);
            tracing::info!("axon result: {}", stdout.trim());
            if let Some(last_line) = stdout.lines().last() {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(last_line) {
                    b.2.processed = v["processed"].as_u64().unwrap_or(0) as u32;
                    b.2.failed = v["failed"].as_u64().unwrap_or(0) as u32;
                    if let Some(e) = v["elapsed_s"].as_f64() {
                        b.2.elapsed_s = e;
                    }
                }
            }
        }
        b.0 = "done".into();
    }
    snap_task.abort();
}

async fn collect_snapshots(bench: Arc<RwLock<(String, BenchRun, BenchRun)>>, hw: SharedState) {
    let start = std::time::Instant::now();
    loop {
        tokio::time::sleep(tokio::time::Duration::from_millis(400)).await;
        let t = start.elapsed().as_secs_f64();
        let (cpu, ram, pressure) = {
            let g = hw.lock().unwrap();
            (
                g.hw.cpu_usage_pct,
                g.hw.ram_used_gb,
                format!("{:?}", g.hw.ram_pressure).to_lowercase(),
            )
        };
        let snap = BenchSnap {
            t,
            cpu,
            ram_gb: ram,
            pressure,
        };
        let mut b = bench.write().await;
        match b.0.as_str() {
            "naive_running" => b.1.snapshots.push(snap),
            "axon_running" => b.2.snapshots.push(snap),
            "done" => break,
            _ => {}
        }
    }
}

fn find_bench_sh() -> String {
    // Check relative paths from likely working directories
    for p in &[
        "dashboard/bench.sh",
        "../dashboard/bench.sh",
        "crates/axon-cli/../../dashboard/bench.sh",
    ] {
        if std::path::Path::new(p).exists() {
            return std::fs::canonicalize(p)
                .map(|p| p.to_string_lossy().to_string())
                .unwrap_or_else(|_| p.to_string());
        }
    }
    // Absolute fallback
    let home = dirs::home_dir().unwrap_or_default();
    let abs = home.join("github/axon/dashboard/bench.sh");
    tracing::info!("bench.sh resolved to: {}", abs.display());
    abs.to_string_lossy().to_string()
}

// ── Public Entry Point ───────────────────────────────────────────────────────

pub async fn run_dashboard(state: SharedState, db: DbHandle, port: u16) -> anyhow::Result<()> {
    let dashboard_state = DashboardState {
        hw_state: state,
        db,
        interactions: Arc::new(RwLock::new(Vec::new())),
        spawned_agents: Arc::new(RwLock::new(Vec::new())),
        bench: Arc::new(RwLock::new((
            "idle".into(),
            BenchRun::default(),
            BenchRun::default(),
        ))),
    };

    let app = Router::new()
        .route("/", get(serve_index))
        .route("/api/snapshot", get(api_snapshot))
        .route("/api/profile", get(api_profile))
        .route("/api/alerts", get(api_alerts))
        .route("/api/agents", get(api_agents))
        .route("/api/interactions", get(api_interactions))
        .route("/api/agent-status", get(api_agent_status))
        .route("/api/spawn-agents", post(api_spawn_agents))
        .route("/api/bench/start", post(api_bench_start))
        .route("/api/bench/status", get(api_bench_status))
        .with_state(dashboard_state);

    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    tracing::info!("dashboard listening on http://{}", addr);
    eprintln!("Dashboard: http://{}", addr);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;

    Ok(())
}
