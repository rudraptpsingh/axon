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
    types::McpResponse,
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
            name: "Fan-out Governor".into(),
            prompt: "You have access to axon MCP tools. Call agent_runtime_health to inspect \
                     Codex/Claude/Cursor/MCP process accumulation. Then call workload_advice \
                     for browser_test with requested_parallelism=4. Decide whether to spawn \
                     another local MCP-heavy browser worker, cap parallelism, reuse an existing \
                     tool server, or clean up first.".into(),
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

async fn api_runtime_health() -> Json<axon_core::types::AgentRuntimeHealth> {
    Json(axon_core::agent_runtime::scan_agent_runtime_health())
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

        // Run a local, deterministic agent simulation by default. The dashboard
        // must work in air-gapped demos and disabled API accounts.
        let interactions = state.interactions.clone();
        let spawned_agents = state.spawned_agents.clone();
        let hw_state = state.hw_state.clone();
        let agent_id = id.clone();
        let agent_name = task.name.clone();
        let prompt = task.prompt.clone();

        tokio::spawn(async move {
            run_local_axon_agent(
                agent_id,
                agent_name,
                prompt,
                hw_state,
                interactions,
                spawned_agents,
            )
            .await;
        });

        agent_ids.push(id);
    }

    Json(SpawnResponse {
        spawned: agent_ids.len(),
        agent_ids,
    })
}

// ── Local Dashboard Agent Runner ─────────────────────────────────────────────

async fn run_local_axon_agent(
    agent_id: String,
    agent_name: String,
    _prompt: String,
    hw_state: SharedState,
    interactions: Arc<RwLock<Vec<Interaction>>>,
    spawned_agents: Arc<RwLock<Vec<SpawnedAgent>>>,
) {
    let now = || chrono::Utc::now().to_rfc3339();

    push_interaction(
        &interactions,
        Interaction {
            ts: now(),
            agent_id: agent_id.clone(),
            agent_name: agent_name.clone(),
            direction: "agent_decision".into(),
            tool: None,
            params: None,
            response: None,
            message: Some(format!("Local Axon agent starting: {}", agent_name)),
            input_tokens: None,
            output_tokens: None,
        },
    )
    .await;

    if agent_name == "System Checker" {
        push_tool_call(&interactions, &agent_id, &agent_name, "hw_snapshot", "{}").await;
        let (hw_response, blame_response, decision) = {
            let guard = hw_state.lock().unwrap();
            let headroom = format!("{:?}", guard.hw.headroom).to_lowercase();
            let impact = format!("{:?}", guard.hw.impact_level).to_lowercase();
            let hw_response = dashboard_tool_response(
                guard.hw.clone(),
                format!(
                    "Headroom is {}. CPU {:.0}%, RAM {:.1}/{:.0}GB. {}",
                    headroom,
                    guard.hw.cpu_usage_pct,
                    guard.hw.ram_used_gb,
                    guard.hw.ram_total_gb,
                    guard.hw.headroom_reason
                ),
            );
            let blame_response = dashboard_tool_response(
                guard.blame.clone(),
                format!(
                    "Top culprit: {}. Impact: {}. Fix: {}",
                    guard.hw.top_culprit, impact, guard.blame.fix
                ),
            );
            let decision = if headroom == "insufficient" {
                format!(
                    "Defer heavy build/test fan-out. Headroom is insufficient because {}. Ask the user to clean up or run with lower parallelism first.",
                    guard.hw.headroom_reason
                )
            } else if headroom == "limited" {
                format!(
                    "Proceed cautiously with capped parallelism. Current culprit: {}. Recommended fix: {}",
                    guard.hw.top_culprit, guard.blame.fix
                )
            } else {
                "Proceed. Host has adequate headroom for the next heavy task.".to_string()
            };
            (hw_response, blame_response, decision)
        };
        push_tool_result(
            &interactions,
            &agent_id,
            &agent_name,
            "hw_snapshot",
            &hw_response,
        )
        .await;
        push_tool_call(&interactions, &agent_id, &agent_name, "process_blame", "{}").await;
        push_tool_result(
            &interactions,
            &agent_id,
            &agent_name,
            "process_blame",
            &blame_response,
        )
        .await;
        push_decision(&interactions, &agent_id, &agent_name, decision).await;
    } else {
        push_tool_call(
            &interactions,
            &agent_id,
            &agent_name,
            "agent_runtime_health",
            "{}",
        )
        .await;
        let runtime = axon_core::agent_runtime::scan_agent_runtime_health();
        let runtime_narrative = crate::agent_runtime_health_narrative_pub(&runtime);
        let runtime_response = dashboard_tool_response(runtime.clone(), runtime_narrative.clone());
        push_tool_result(
            &interactions,
            &agent_id,
            &agent_name,
            "agent_runtime_health",
            &runtime_response,
        )
        .await;

        push_tool_call(
            &interactions,
            &agent_id,
            &agent_name,
            "workload_advice",
            "{\"kind\":\"browser_test\",\"requested_parallelism\":4}",
        )
        .await;
        let advice_response = {
            let guard = hw_state.lock().unwrap();
            let request = axon_core::types::WorkloadAdviceRequest {
                kind: axon_core::types::WorkloadKind::BrowserTest,
                requested_parallelism: Some(4),
                estimated_duration_s: None,
                gpu_required: false,
            };
            let advice = axon_core::impact::advise_workload(
                &request,
                &guard.hw,
                &guard.blame,
                guard.gpu.as_ref(),
                &guard.profile,
            );
            let narrative = crate::workload_advice_narrative_pub(&advice);
            dashboard_tool_response(advice, narrative)
        };
        push_tool_result(
            &interactions,
            &agent_id,
            &agent_name,
            "workload_advice",
            &advice_response,
        )
        .await;

        let decision = if runtime.mcp_server_count >= 8
            || !runtime.duplicate_mcp_server_groups.is_empty()
        {
            format!(
                "Do not spawn another MCP-heavy browser worker. Reuse an existing tool server or clean up first: {} MCP servers, duplicate groups: {}.",
                runtime.mcp_server_count,
                runtime.duplicate_mcp_server_groups.join(", ")
            )
        } else {
            format!(
                "Proceed with browser automation. Runtime footprint is acceptable: {} MCP servers and {:.0}MB agent RAM.",
                runtime.mcp_server_count, runtime.total_ram_mb
            )
        };
        push_decision(&interactions, &agent_id, &agent_name, decision).await;
    }

    {
        let mut agents = spawned_agents.write().await;
        if let Some(a) = agents.iter_mut().find(|a| a.id == agent_id) {
            a.status = "completed".into();
            a.input_tokens = 180;
            a.output_tokens = 92;
        }
    }
}

fn dashboard_tool_response<T: Serialize + Clone>(data: T, narrative: String) -> String {
    serde_json::to_string(&McpResponse::success(data, narrative))
        .unwrap_or_else(|e| format!("{{\"ok\":false,\"error\":\"{}\"}}", e))
}

async fn push_tool_call(
    interactions: &Arc<RwLock<Vec<Interaction>>>,
    agent_id: &str,
    agent_name: &str,
    tool: &str,
    params: &str,
) {
    push_interaction(
        interactions,
        Interaction {
            ts: chrono::Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            direction: "agent_to_axon".into(),
            tool: Some(tool.into()),
            params: Some(params.into()),
            response: None,
            message: None,
            input_tokens: None,
            output_tokens: None,
        },
    )
    .await;
}

async fn push_tool_result(
    interactions: &Arc<RwLock<Vec<Interaction>>>,
    agent_id: &str,
    agent_name: &str,
    tool: &str,
    response: &str,
) {
    push_interaction(
        interactions,
        Interaction {
            ts: chrono::Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            direction: "axon_to_agent".into(),
            tool: Some(tool.into()),
            params: None,
            response: Some(truncate(response, 360)),
            message: None,
            input_tokens: None,
            output_tokens: None,
        },
    )
    .await;
}

async fn push_decision(
    interactions: &Arc<RwLock<Vec<Interaction>>>,
    agent_id: &str,
    agent_name: &str,
    message: String,
) {
    push_interaction(
        interactions,
        Interaction {
            ts: chrono::Utc::now().to_rfc3339(),
            agent_id: agent_id.into(),
            agent_name: agent_name.into(),
            direction: "agent_decision".into(),
            tool: None,
            params: None,
            response: None,
            message: Some(message),
            input_tokens: Some(180),
            output_tokens: Some(92),
        },
    )
    .await;
}

async fn push_interaction(interactions: &Arc<RwLock<Vec<Interaction>>>, interaction: Interaction) {
    let mut ints = interactions.write().await;
    ints.push(interaction);
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

    let claude_code_path = home.join(".claude.json");
    agents.push(check_agent_config(
        "Claude Code",
        &claude_code_path.to_string_lossy(),
    ));

    let cursor_path = home.join(".cursor/mcp.json");
    agents.push(check_agent_config("Cursor", &cursor_path.to_string_lossy()));

    #[cfg(target_os = "macos")]
    let vscode_path = home.join("Library/Application Support/Code/User/settings.json");
    #[cfg(target_os = "linux")]
    let vscode_path = home.join(".config/Code/User/settings.json");
    #[cfg(target_os = "windows")]
    let vscode_path = home.join("AppData/Roaming/Code/User/settings.json");
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
        .route("/api/runtime-health", get(api_runtime_health))
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
