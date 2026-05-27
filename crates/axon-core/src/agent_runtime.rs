use std::collections::HashMap;

use sysinfo::{System, MINIMUM_CPU_UPDATE_INTERVAL};

use crate::types::{
    AgentRuntimeHealth, AgentRuntimeImpact, AgentRuntimeProcess, AgentRuntimeProvider,
    AgentRuntimeRole,
};

const STALE_RUNTIME_SECS: u64 = 4 * 60 * 60;

pub fn scan_agent_runtime_health() -> AgentRuntimeHealth {
    let mut sys = System::new_all();
    sys.refresh_all();
    std::thread::sleep(MINIMUM_CPU_UPDATE_INTERVAL);
    sys.refresh_all();

    let mut processes: Vec<AgentRuntimeProcess> = sys
        .processes()
        .values()
        .filter_map(|process| {
            let name = process.name().to_string_lossy().into_owned();
            let cmd = process
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy())
                .collect::<Vec<_>>()
                .join(" ");
            classify_runtime(&name, &cmd).map(|(provider, role)| {
                let uptime_s = process.run_time();
                AgentRuntimeProcess {
                    pid: usize::from(process.pid()) as u32,
                    ppid: process.parent().map(|p| usize::from(p) as u32),
                    provider,
                    role,
                    name: display_name(&name, &cmd),
                    cpu_pct: process.cpu_usage() as f64,
                    ram_mb: process.memory() as f64 / 1_048_576.0,
                    uptime_s,
                    stale: uptime_s >= STALE_RUNTIME_SECS,
                }
            })
        })
        .collect();

    processes.sort_by(|a, b| {
        b.cpu_pct
            .partial_cmp(&a.cpu_pct)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                b.ram_mb
                    .partial_cmp(&a.ram_mb)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
    });

    let process_count = processes.len() as u32;
    let stale_process_count = processes.iter().filter(|p| p.stale).count() as u32;
    let mcp_server_count = processes
        .iter()
        .filter(|p| p.role == AgentRuntimeRole::McpServer)
        .count() as u32;
    let stale_mcp_server_count = processes
        .iter()
        .filter(|p| p.role == AgentRuntimeRole::McpServer && p.stale)
        .count() as u32;
    let mcp_total_ram_mb = processes
        .iter()
        .filter(|p| p.role == AgentRuntimeRole::McpServer)
        .map(|p| p.ram_mb)
        .sum();
    let orphaned_mcp_server_count = processes
        .iter()
        .filter(|p| p.role == AgentRuntimeRole::McpServer && p.ppid == Some(1))
        .count() as u32;
    let duplicate_mcp_server_groups = duplicate_mcp_server_groups(&processes);
    let renderer_cpu_pct = role_cpu_pct(&processes, AgentRuntimeRole::Renderer);
    let gpu_helper_cpu_pct = role_cpu_pct(&processes, AgentRuntimeRole::GpuHelper);
    let high_cpu_ui_process_count = processes
        .iter()
        .filter(|p| {
            matches!(
                p.role,
                AgentRuntimeRole::Renderer | AgentRuntimeRole::GpuHelper
            ) && p.cpu_pct >= 25.0
        })
        .count() as u32;
    let total_ram_mb = processes.iter().map(|p| p.ram_mb).sum();
    let total_cpu_pct = processes.iter().map(|p| p.cpu_pct).sum();
    let codex_process_count = processes
        .iter()
        .filter(|p| p.provider == AgentRuntimeProvider::Codex)
        .count() as u32;
    let codex_stale_process_count = processes
        .iter()
        .filter(|p| p.provider == AgentRuntimeProvider::Codex && p.stale)
        .count() as u32;
    let claude_process_count = processes
        .iter()
        .filter(|p| p.provider == AgentRuntimeProvider::Claude)
        .count() as u32;
    let cursor_process_count = processes
        .iter()
        .filter(|p| p.provider == AgentRuntimeProvider::Cursor)
        .count() as u32;

    let stale_processes = processes
        .iter()
        .filter(|p| p.stale)
        .take(20)
        .cloned()
        .collect::<Vec<_>>();
    let top_processes = processes.iter().take(12).cloned().collect::<Vec<_>>();
    let workflow_impacts = workflow_impacts(AgentRuntimeSignals {
        stale_process_count,
        mcp_server_count,
        orphaned_mcp_server_count,
        duplicate_mcp_server_groups: &duplicate_mcp_server_groups,
        stale_mcp_server_count,
        mcp_total_ram_mb,
        renderer_cpu_pct,
        gpu_helper_cpu_pct,
        high_cpu_ui_process_count,
        codex_stale_process_count,
        total_ram_mb,
        total_cpu_pct,
    });

    let recommendations = recommendations(
        process_count,
        stale_process_count,
        mcp_server_count,
        total_ram_mb,
        total_cpu_pct,
        codex_stale_process_count,
        orphaned_mcp_server_count,
        &duplicate_mcp_server_groups,
        stale_mcp_server_count,
        mcp_total_ram_mb,
        renderer_cpu_pct,
        gpu_helper_cpu_pct,
        high_cpu_ui_process_count,
    );

    AgentRuntimeHealth {
        process_count,
        stale_process_count,
        mcp_server_count,
        orphaned_mcp_server_count,
        duplicate_mcp_server_groups,
        stale_mcp_server_count,
        mcp_total_ram_mb,
        renderer_cpu_pct,
        gpu_helper_cpu_pct,
        high_cpu_ui_process_count,
        total_ram_mb,
        total_cpu_pct,
        codex_process_count,
        codex_stale_process_count,
        claude_process_count,
        cursor_process_count,
        top_processes,
        stale_processes,
        workflow_impacts,
        recommendations,
    }
}

struct AgentRuntimeSignals<'a> {
    stale_process_count: u32,
    mcp_server_count: u32,
    orphaned_mcp_server_count: u32,
    duplicate_mcp_server_groups: &'a [String],
    stale_mcp_server_count: u32,
    mcp_total_ram_mb: f64,
    renderer_cpu_pct: f64,
    gpu_helper_cpu_pct: f64,
    high_cpu_ui_process_count: u32,
    codex_stale_process_count: u32,
    total_ram_mb: f64,
    total_cpu_pct: f64,
}

pub fn classify_runtime(
    process_name: &str,
    cmdline: &str,
) -> Option<(AgentRuntimeProvider, AgentRuntimeRole)> {
    let name = process_name.to_ascii_lowercase();
    let cmd = cmdline.to_ascii_lowercase();
    let hay = format!("{name} {cmd}");

    if hay.contains("node_repl")
        || hay.contains("figbridge-mcp")
        || hay.contains("playwright-mcp")
        || hay.contains("skycomputeruseclient")
        || hay.contains("oversight/dist/mcp")
        || (hay.contains(" mcp") && hay.contains("codex"))
    {
        return Some((AgentRuntimeProvider::Codex, AgentRuntimeRole::McpServer));
    }

    if hay.contains("codex.app")
        || hay.contains("/codex app-server")
        || hay.contains("com.openai.codex")
    {
        let role = if hay.contains("--analytics-default-enabled") {
            AgentRuntimeRole::AppServer
        } else if hay.contains("--listen stdio://") {
            AgentRuntimeRole::AppServer
        } else if hay.contains("--type=renderer") {
            AgentRuntimeRole::Renderer
        } else if hay.contains("--type=gpu-process") {
            AgentRuntimeRole::GpuHelper
        } else if hay.contains("sparkle") || hay.contains("updater.app") {
            AgentRuntimeRole::Updater
        } else {
            AgentRuntimeRole::HostApp
        };
        return Some((AgentRuntimeProvider::Codex, role));
    }

    if let Some(role) = classify_claude_runtime(&name, &cmd) {
        return Some((AgentRuntimeProvider::Claude, role));
    }

    if let Some(role) = classify_cursor_runtime(&name, &cmd) {
        return Some((AgentRuntimeProvider::Cursor, role));
    }
    if is_windsurf_runtime(&name, &cmd) {
        return Some((AgentRuntimeProvider::Windsurf, AgentRuntimeRole::HostApp));
    }
    if is_zed_runtime(&name, &cmd) {
        return Some((AgentRuntimeProvider::Zed, AgentRuntimeRole::HostApp));
    }

    if is_generic_mcp_runtime(&name, &cmd) {
        return Some((
            AgentRuntimeProvider::GenericMcp,
            AgentRuntimeRole::McpServer,
        ));
    }

    None
}

fn classify_cursor_runtime(name: &str, cmd: &str) -> Option<AgentRuntimeRole> {
    let is_cursor = name == "cursor"
        || name.starts_with("cursor helper")
        || name.starts_with("cursor-agent")
        || cmd.contains("/cursor.app/")
        || cmd.contains("com.todesktop.230313mzl4w4u92")
        || cmd.contains("/cursor-agent");
    if !is_cursor {
        return None;
    }

    Some(if name.contains("mcp-process") || cmd.contains("mcp") {
        AgentRuntimeRole::McpServer
    } else if name.contains("terminal pty-host") || cmd.contains("pty-host") {
        AgentRuntimeRole::ToolWorker
    } else if name.contains("shared-process") {
        AgentRuntimeRole::AppServer
    } else if cmd.contains("--type=renderer") {
        AgentRuntimeRole::Renderer
    } else if cmd.contains("--type=gpu-process") || name.contains("gpu") {
        AgentRuntimeRole::GpuHelper
    } else {
        AgentRuntimeRole::HostApp
    })
}

fn classify_claude_runtime(name: &str, cmd: &str) -> Option<AgentRuntimeRole> {
    if name.contains("claude") || cmd.contains("claude-daemon") {
        return Some(if cmd.contains("mcp") {
            AgentRuntimeRole::McpServer
        } else {
            AgentRuntimeRole::AppServer
        });
    }

    if cmd.contains("/private/tmp/claude-")
        || cmd.contains("/tmp/claude-")
        || cmd.contains("/.claude/shell-snapshots/")
    {
        return Some(AgentRuntimeRole::ToolWorker);
    }

    None
}

fn is_windsurf_runtime(name: &str, cmd: &str) -> bool {
    name.contains("windsurf") || cmd.contains("/windsurf.app/")
}

fn is_zed_runtime(name: &str, cmd: &str) -> bool {
    name == "zed"
        || name.starts_with("zed ")
        || name.starts_with("zed-")
        || name.starts_with("zed:")
        || cmd.contains("/zed.app/")
        || cmd.contains("/zed/")
        || cmd.starts_with("zed ")
}

fn is_generic_mcp_runtime(name: &str, cmd: &str) -> bool {
    let runtime = name.starts_with("python")
        || name.starts_with("node")
        || name.starts_with("bun")
        || name.starts_with("deno")
        || name.starts_with("uv")
        || name.starts_with("npm");
    runtime
        && (cmd.contains("mcp")
            || cmd.contains("model-context")
            || cmd.contains("modelcontextprotocol"))
}

fn display_name(process_name: &str, cmd: &str) -> String {
    if cmd.contains("figbridge-mcp") {
        "figbridge-mcp".to_string()
    } else if cmd.contains("playwright-mcp") {
        "playwright-mcp".to_string()
    } else if cmd.contains("node_repl") {
        "node_repl".to_string()
    } else if cmd.contains("SkyComputerUseClient") {
        "computer-use-mcp".to_string()
    } else if cmd.contains("oversight/dist/mcp") {
        "oversight-mcp".to_string()
    } else if cmd.contains("codex app-server") {
        "codex app-server".to_string()
    } else {
        process_name.to_string()
    }
}

fn duplicate_mcp_server_groups(processes: &[AgentRuntimeProcess]) -> Vec<String> {
    let mut counts: HashMap<String, u32> = HashMap::new();
    for process in processes
        .iter()
        .filter(|p| p.role == AgentRuntimeRole::McpServer)
    {
        *counts.entry(process.name.clone()).or_insert(0) += 1;
    }

    let mut groups = counts
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(name, count)| format!("{name} x{count}"))
        .collect::<Vec<_>>();
    groups.sort();
    groups
}

fn role_cpu_pct(processes: &[AgentRuntimeProcess], role: AgentRuntimeRole) -> f64 {
    processes
        .iter()
        .filter(|p| p.role == role)
        .map(|p| p.cpu_pct)
        .sum()
}

fn workflow_impacts(signals: AgentRuntimeSignals<'_>) -> Vec<AgentRuntimeImpact> {
    let mut impacts = Vec::new();

    if signals.mcp_server_count >= 12
        || signals.stale_mcp_server_count >= 8
        || !signals.duplicate_mcp_server_groups.is_empty()
        || signals.orphaned_mcp_server_count > 0
    {
        impacts.push(AgentRuntimeImpact {
            use_case: "Long agent coding session".to_string(),
            visible_symptom: format!(
                "{} MCP/tool servers, {} stale, duplicate groups: {}",
                signals.mcp_server_count,
                signals.stale_mcp_server_count,
                if signals.duplicate_mcp_server_groups.is_empty() {
                    "none".to_string()
                } else {
                    signals.duplicate_mcp_server_groups.join(", ")
                }
            ),
            business_impact: "Prevents slowdowns where every new prompt, tool call, or file edit competes with stale tool servers from old sessions.".to_string(),
            recommended_action: "Save work, restart the agent host, then reopen only the active workspace before launching more subagents or MCP-heavy tools.".to_string(),
        });
    }

    if signals.renderer_cpu_pct >= 10.0
        || signals.gpu_helper_cpu_pct >= 10.0
        || signals.high_cpu_ui_process_count > 0
    {
        impacts.push(AgentRuntimeImpact {
            use_case: "Interactive IDE or desktop-agent work".to_string(),
            visible_symptom: format!(
                "Renderer {:.0}% CPU, GPU helper {:.0}% CPU",
                signals.renderer_cpu_pct, signals.gpu_helper_cpu_pct
            ),
            business_impact: "Keeps the editor responsive during reviews, diffs, chat, and browser automation instead of letting UI helpers steal CPU from builds/tests.".to_string(),
            recommended_action: "Restart or hide the desktop agent UI before heavy local work; if it returns immediately, disable the extension or renderer-heavy view causing the spin.".to_string(),
        });
    }

    if signals.codex_stale_process_count >= 4 || signals.stale_process_count >= 8 {
        impacts.push(AgentRuntimeImpact {
            use_case: "Multi-session agent workspace".to_string(),
            visible_symptom: format!(
                "{} stale runtime processes, {} stale Codex processes",
                signals.stale_process_count, signals.codex_stale_process_count
            ),
            business_impact: "Reduces failed or flaky runs caused by old sessions consuming file handles, memory, terminal slots, and MCP connections.".to_string(),
            recommended_action: "Cleanly restart the agent app after saving work, then rerun Axon to confirm stale runtime count dropped.".to_string(),
        });
    }

    if signals.total_cpu_pct >= 50.0
        || signals.total_ram_mb >= 1_000.0
        || signals.mcp_total_ram_mb >= 1_000.0
    {
        impacts.push(AgentRuntimeImpact {
            use_case: "Build, test, Docker, and browser automation preflight".to_string(),
            visible_symptom: format!(
                "Agent runtime load {:.0}% CPU, {:.0}MB RAM, MCP RAM {:.0}MB",
                signals.total_cpu_pct, signals.total_ram_mb, signals.mcp_total_ram_mb
            ),
            business_impact: "Avoids starting expensive work when the host is already saturated, which saves failed CI-like reruns and developer wait time.".to_string(),
            recommended_action: "Use workload_advice before the next heavy job and cap parallelism until runtime load drops.".to_string(),
        });
    }

    if impacts.is_empty() {
        impacts.push(AgentRuntimeImpact {
            use_case: "Agent readiness check".to_string(),
            visible_symptom: "No agent-runtime signal is currently above the action threshold."
                .to_string(),
            business_impact: "Gives confidence that the local machine is ready for another agent task without cleanup first.".to_string(),
            recommended_action: "Proceed normally; call workload_advice before heavy parallel work.".to_string(),
        });
    }

    impacts
}

fn recommendations(
    process_count: u32,
    stale_process_count: u32,
    mcp_server_count: u32,
    total_ram_mb: f64,
    total_cpu_pct: f64,
    codex_stale_process_count: u32,
    orphaned_mcp_server_count: u32,
    duplicate_mcp_server_groups: &[String],
    stale_mcp_server_count: u32,
    mcp_total_ram_mb: f64,
    renderer_cpu_pct: f64,
    gpu_helper_cpu_pct: f64,
    high_cpu_ui_process_count: u32,
) -> Vec<String> {
    let mut out = Vec::new();
    if high_cpu_ui_process_count > 0 || renderer_cpu_pct >= 50.0 || gpu_helper_cpu_pct >= 30.0 {
        out.push(format!(
            "Agent UI process pressure detected: renderer {:.0}% CPU, GPU helper {:.0}% CPU; restart or hide the desktop app before heavy local work.",
            renderer_cpu_pct, gpu_helper_cpu_pct
        ));
    }
    if orphaned_mcp_server_count > 0 {
        out.push(format!(
            "{orphaned_mcp_server_count} MCP servers are orphaned under PID 1; restart the owning agent or clean up stale MCP processes."
        ));
    }
    if !duplicate_mcp_server_groups.is_empty() {
        out.push(format!(
            "Duplicate MCP server groups detected: {}; check for per-session MCP stack duplication.",
            duplicate_mcp_server_groups.join(", ")
        ));
    }
    if stale_process_count >= 8 {
        out.push(format!(
            "{stale_process_count} agent runtime processes are older than 4h; restart the agent host to release stale tool servers."
        ));
    }
    if codex_stale_process_count >= 4 {
        out.push(format!(
            "{codex_stale_process_count} stale Codex runtime processes detected; restart Codex after saving work."
        ));
    }
    if mcp_server_count >= 12 {
        out.push(format!(
            "{mcp_server_count} MCP servers are running; avoid spawning more tools/subagents until cleanup."
        ));
    }
    if stale_mcp_server_count >= 8 {
        out.push(format!(
            "{stale_mcp_server_count} MCP servers are older than 4h; prefer a clean agent restart over opening more sessions."
        ));
    }
    if mcp_total_ram_mb >= 1_000.0 {
        out.push(format!(
            "MCP servers alone are using {:.0}MB RAM; close duplicated browser/tool MCP stacks.",
            mcp_total_ram_mb
        ));
    }
    if total_ram_mb >= 1_000.0 {
        out.push(format!(
            "Agent runtimes are using {:.0}MB RAM; close stale sessions before heavy work.",
            total_ram_mb
        ));
    }
    if total_cpu_pct >= 50.0 {
        out.push(format!(
            "Agent runtimes are using {:.0}% CPU; defer builds/tests until CPU settles.",
            total_cpu_pct
        ));
    }
    if out.is_empty() && process_count > 0 {
        out.push(
            "Agent runtime footprint is present but no immediate cleanup is required.".to_string(),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_codex_app_server() {
        let (provider, role) = classify_runtime(
            "codex",
            "/Applications/Codex.app/Contents/Resources/codex app-server --analytics-default-enabled",
        )
        .unwrap();
        assert_eq!(provider, AgentRuntimeProvider::Codex);
        assert_eq!(role, AgentRuntimeRole::AppServer);
    }

    #[test]
    fn classifies_codex_mcp_tools() {
        let (provider, role) =
            classify_runtime("node", "node /tmp/node_modules/.bin/playwright-mcp").unwrap();
        assert_eq!(provider, AgentRuntimeProvider::Codex);
        assert_eq!(role, AgentRuntimeRole::McpServer);
    }

    #[test]
    fn classifies_generic_mcp_runtime() {
        let (provider, role) =
            classify_runtime("python3", "python3 server.py --model-context-protocol").unwrap();
        assert_eq!(provider, AgentRuntimeProvider::GenericMcp);
        assert_eq!(role, AgentRuntimeRole::McpServer);
    }

    #[test]
    fn classifies_claude_shell_helpers_as_tool_workers() {
        let (provider, role) = classify_runtime(
            "zsh",
            "/bin/zsh -c source /Users/rp/.claude/shell-snapshots/snapshot.sh && tail /private/tmp/claude-501/task.output",
        )
        .unwrap();
        assert_eq!(provider, AgentRuntimeProvider::Claude);
        assert_eq!(role, AgentRuntimeRole::ToolWorker);
    }

    #[test]
    fn classifies_cursor_mcp_process() {
        let (provider, role) =
            classify_runtime("Cursor Helper: mcp-process", "Cursor Helper: mcp-process").unwrap();
        assert_eq!(provider, AgentRuntimeProvider::Cursor);
        assert_eq!(role, AgentRuntimeRole::McpServer);
    }

    #[test]
    fn does_not_classify_apple_cursor_ui_service_as_cursor() {
        assert_eq!(
            classify_runtime(
                "CursorUIViewService",
                "/System/Library/PrivateFrameworks/TextInputUIMacHelper.framework/Versions/A/XPCServices/CursorUIViewService.xpc/Contents/MacOS/CursorUIViewService"
            ),
            None
        );
    }

    #[test]
    fn reports_duplicate_mcp_server_groups() {
        let processes = vec![
            AgentRuntimeProcess {
                pid: 1,
                ppid: Some(10),
                provider: AgentRuntimeProvider::Codex,
                role: AgentRuntimeRole::McpServer,
                name: "playwright-mcp".to_string(),
                cpu_pct: 0.0,
                ram_mb: 30.0,
                uptime_s: 10,
                stale: false,
            },
            AgentRuntimeProcess {
                pid: 2,
                ppid: Some(10),
                provider: AgentRuntimeProvider::Codex,
                role: AgentRuntimeRole::McpServer,
                name: "playwright-mcp".to_string(),
                cpu_pct: 0.0,
                ram_mb: 30.0,
                uptime_s: 10,
                stale: false,
            },
        ];

        assert_eq!(
            duplicate_mcp_server_groups(&processes),
            vec!["playwright-mcp x2".to_string()]
        );
    }

    #[test]
    fn computes_role_cpu_totals() {
        let processes = vec![
            AgentRuntimeProcess {
                pid: 1,
                ppid: Some(10),
                provider: AgentRuntimeProvider::Codex,
                role: AgentRuntimeRole::Renderer,
                name: "Codex Helper (Renderer)".to_string(),
                cpu_pct: 75.0,
                ram_mb: 200.0,
                uptime_s: 10,
                stale: false,
            },
            AgentRuntimeProcess {
                pid: 2,
                ppid: Some(10),
                provider: AgentRuntimeProvider::Codex,
                role: AgentRuntimeRole::GpuHelper,
                name: "Codex Helper".to_string(),
                cpu_pct: 12.0,
                ram_mb: 40.0,
                uptime_s: 10,
                stale: false,
            },
        ];

        assert_eq!(role_cpu_pct(&processes, AgentRuntimeRole::Renderer), 75.0);
        assert_eq!(role_cpu_pct(&processes, AgentRuntimeRole::GpuHelper), 12.0);
    }

    #[test]
    fn workflow_impacts_translate_duplicate_mcp_into_use_case() {
        let duplicate_groups = vec!["playwright-mcp x3".to_string()];
        let impacts = workflow_impacts(AgentRuntimeSignals {
            stale_process_count: 10,
            mcp_server_count: 14,
            orphaned_mcp_server_count: 0,
            duplicate_mcp_server_groups: &duplicate_groups,
            stale_mcp_server_count: 9,
            mcp_total_ram_mb: 600.0,
            renderer_cpu_pct: 0.0,
            gpu_helper_cpu_pct: 0.0,
            high_cpu_ui_process_count: 0,
            codex_stale_process_count: 6,
            total_ram_mb: 700.0,
            total_cpu_pct: 10.0,
        });

        assert!(impacts
            .iter()
            .any(|impact| impact.use_case == "Long agent coding session"));
        assert!(impacts
            .iter()
            .any(|impact| impact.business_impact.contains("new prompt")));
    }

    #[test]
    fn does_not_classify_unrelated_localized_process_as_zed() {
        assert_eq!(
            classify_runtime(
                "WhatsApp",
                "/Applications/WhatsApp.app/Contents/Frameworks/Electron Framework.framework/Helpers/chrome_crashpad_handler --monitor-self --database=/Users/rp/Library/Application Support/WhatsApp/Crashpad --annotation=plat=OS X --annotation=prod=Electron --annotation=ver=35.7.5 --handshake-fd=30"
            ),
            None
        );
    }
}
