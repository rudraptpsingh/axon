use std::collections::HashMap;

use crate::types::{ProcessGroup, ProcessInfo};

/// Normalize a process name to its app-level group name.
/// Strips helper suffixes, path prefixes, and maps known executables to display names.
pub fn normalize_process_name(cmd: &str) -> String {
    let name = cmd.trim_end_matches('\0').trim();

    // Strip path prefix (e.g., "/usr/bin/node" -> "node", "C:\Program Files\app.exe" -> "app.exe")
    let base = name.rsplit(['/', '\\']).next().unwrap_or(name);

    // Strip .exe suffix (Windows)
    let base = base.strip_suffix(".exe").unwrap_or(base);

    // Strip helper suffixes common in macOS apps
    let stripped = base
        .trim_end_matches(" (Renderer)")
        .trim_end_matches(" (GPU)")
        .trim_end_matches(" (Plugin)")
        .trim_end_matches(" Helper")
        .trim();

    if stripped.is_empty() {
        return name.to_string();
    }

    stripped.to_string()
}

/// Group a list of processes by normalized app name.
/// Returns groups sorted by blame_score descending (highest-blame group first).
pub fn build_groups(processes: &[ProcessInfo]) -> Vec<ProcessGroup> {
    let mut map: HashMap<String, ProcessGroup> = HashMap::new();

    for p in processes {
        let group_name = normalize_process_name(&p.cmd);
        let entry = map
            .entry(group_name.clone())
            .or_insert_with(|| ProcessGroup {
                name: group_name,
                process_count: 0,
                total_cpu_pct: 0.0,
                total_ram_gb: 0.0,
                blame_score: 0.0,
                top_pid: p.pid,
                pids: Vec::new(),
            });

        entry.process_count += 1;
        entry.total_cpu_pct += p.cpu_pct;
        entry.total_ram_gb += p.ram_gb;
        if p.blame_score > entry.blame_score {
            entry.blame_score = p.blame_score;
            entry.top_pid = p.pid;
        }
        entry.pids.push(p.pid);
    }

    let mut groups: Vec<ProcessGroup> = map.into_values().collect();
    groups.sort_by(|a, b| {
        b.blame_score
            .partial_cmp(&a.blame_score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    groups
}

// ── Claude Sub-Agent Cmdline Parsing ─────────────────────────────────────────

/// Metadata extracted from a claude process's command-line arguments.
#[derive(Debug, Clone, Default)]
pub struct ClaudeCmdlineMeta {
    /// Session ID from --session <value> or --session=<value>.
    pub session_id: Option<String>,
    /// True when --init, --replay-user-messages, or --resume= flags are present
    /// (these only appear on the orchestrator, not sub-agents).
    pub is_orchestrator: bool,
}

/// Extract a `cse_...` session ID embedded in a URL like
/// `https://api.anthropic.com/v1/code/sessions/cse_xxx` or from a
/// mcp-config filename like `/tmp/mcp-config-cse_xxx.json`.
fn extract_session_from_url(s: &str) -> Option<String> {
    // URL pattern: .../sessions/<id>
    if let Some(after) = s.split("/sessions/").nth(1) {
        let id = after
            .split(|c: char| !c.is_alphanumeric() && c != '_' && c != '-')
            .next()?;
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    // mcp-config filename pattern: mcp-config-<id>.json
    if let Some(after) = s.strip_prefix("mcp-config-") {
        let id = after.trim_end_matches(".json");
        if !id.is_empty() {
            return Some(id.to_string());
        }
    }
    None
}

/// Parse claude process metadata from raw cmdline bytes.
/// On Linux /proc/<pid>/cmdline uses null bytes as argument delimiters.
pub fn parse_claude_cmdline(cmdline_bytes: &[u8]) -> ClaudeCmdlineMeta {
    let args: Vec<&str> = cmdline_bytes
        .split(|&b| b == 0)
        .filter(|s| !s.is_empty())
        .filter_map(|s| std::str::from_utf8(s).ok())
        .collect();

    let mut meta = ClaudeCmdlineMeta::default();
    let mut i = 0;
    while i < args.len() {
        let arg = args[i];
        if arg == "--init" || arg == "--replay-user-messages" {
            meta.is_orchestrator = true;
        } else if let Some(url) = arg.strip_prefix("--resume=") {
            meta.is_orchestrator = true;
            if meta.session_id.is_none() {
                meta.session_id = extract_session_from_url(url);
            }
        } else if arg == "--session" {
            if i + 1 < args.len() {
                meta.session_id = Some(args[i + 1].to_string());
                i += 1;
            }
        } else if let Some(val) = arg.strip_prefix("--session=") {
            meta.session_id = Some(val.to_string());
        } else if arg == "--sdk-url" {
            if i + 1 < args.len() {
                if meta.session_id.is_none() {
                    meta.session_id = extract_session_from_url(args[i + 1]);
                }
                i += 1;
            }
        } else if arg == "--mcp-config" && i + 1 < args.len() {
            let path = args[i + 1];
            // Extract basename, then session id from "mcp-config-<id>.json"
            let basename = path.rsplit('/').next().unwrap_or(path);
            if meta.session_id.is_none() {
                meta.session_id = extract_session_from_url(basename);
            }
            i += 1;
        }
        i += 1;
    }
    meta
}

/// Returns true if the process at `pid` has any file descriptor pointing to
/// `/proc/<other_pid>/statm` for a PID that is not `pid` itself.
/// This identifies the SDK memory-monitor subprocess that watches another
/// claude process's memory usage — it is NOT a user-facing sub-agent.
pub fn is_memory_monitor_process(pid: u32) -> bool {
    #[cfg(target_os = "linux")]
    {
        let fd_dir = format!("/proc/{}/fd", pid);
        let entries = match std::fs::read_dir(&fd_dir) {
            Ok(e) => e,
            Err(_) => return false,
        };
        let statm_suffix_self = format!("/proc/{}/statm", pid);
        for entry in entries.flatten() {
            if let Ok(target) = std::fs::read_link(entry.path()) {
                let t = target.to_string_lossy();
                if t.starts_with("/proc/") && t.ends_with("/statm") && t != statm_suffix_self {
                    return true;
                }
            }
        }
        false
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        false
    }
}

/// Read and parse cmdline for a given PID.
/// Returns None on non-Linux platforms or if /proc/<pid>/cmdline is unreadable.
pub fn read_claude_cmdline(pid: u32) -> Option<ClaudeCmdlineMeta> {
    #[cfg(target_os = "linux")]
    {
        let bytes = std::fs::read(format!("/proc/{}/cmdline", pid)).ok()?;
        Some(parse_claude_cmdline(&bytes))
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = pid;
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_strips_helper_suffix() {
        assert_eq!(
            normalize_process_name("Google Chrome Helper (GPU)"),
            "Google Chrome"
        );
        assert_eq!(
            normalize_process_name("Google Chrome Helper (Renderer)"),
            "Google Chrome"
        );
        assert_eq!(
            normalize_process_name("Google Chrome Helper"),
            "Google Chrome"
        );
    }

    #[test]
    fn test_normalize_preserves_simple_name() {
        assert_eq!(normalize_process_name("node"), "node");
        assert_eq!(normalize_process_name("cargo"), "cargo");
        assert_eq!(normalize_process_name("python3"), "python3");
    }

    #[test]
    fn test_normalize_strips_path() {
        assert_eq!(normalize_process_name("/usr/bin/node"), "node");
        assert_eq!(
            normalize_process_name("/Applications/Cursor.app/Contents/MacOS/Cursor Helper (GPU)"),
            "Cursor"
        );
    }

    #[test]
    fn test_normalize_strips_null_bytes() {
        assert_eq!(normalize_process_name("node\0\0"), "node");
    }

    #[test]
    fn test_normalize_strips_windows_path_and_exe() {
        assert_eq!(
            normalize_process_name("C:\\Program Files\\nodejs\\node.exe"),
            "node"
        );
        assert_eq!(
            normalize_process_name("C:\\Windows\\System32\\cmd.exe"),
            "cmd"
        );
        assert_eq!(normalize_process_name("D:\\tools\\cargo.exe"), "cargo");
    }

    #[test]
    fn test_normalize_mixed_separators() {
        // Some tools use forward slashes on Windows
        assert_eq!(
            normalize_process_name("C:/Users/user/.cargo/bin/cargo.exe"),
            "cargo"
        );
    }

    /// Verify every real Cursor process variant (observed on macOS) normalizes
    /// to "Cursor" so they group together correctly.
    #[test]
    fn test_normalize_cursor_all_real_variants() {
        // Short names as reported by sysinfo process.name()
        assert_eq!(normalize_process_name("Cursor Helper (Renderer)"), "Cursor");
        assert_eq!(normalize_process_name("Cursor Helper (GPU)"), "Cursor");
        assert_eq!(normalize_process_name("Cursor Helper (Plugin)"), "Cursor");
        assert_eq!(normalize_process_name("Cursor Helper"), "Cursor");
        assert_eq!(normalize_process_name("Cursor"), "Cursor");

        // Full paths (sysinfo sometimes returns these)
        assert_eq!(
            normalize_process_name(
                "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper (Renderer).app/Contents/MacOS/Cursor Helper (Renderer)"
            ),
            "Cursor"
        );
        assert_eq!(
            normalize_process_name("/Applications/Cursor.app/Contents/MacOS/Cursor"),
            "Cursor"
        );
        assert_eq!(
            normalize_process_name(
                "/Applications/Cursor.app/Contents/Frameworks/Cursor Helper.app/Contents/MacOS/Cursor Helper"
            ),
            "Cursor"
        );
    }

    /// Processes spawned by Cursor but not Cursor itself should NOT normalize to "Cursor".
    #[test]
    fn test_normalize_cursor_child_processes_stay_separate() {
        // Node helper spawned by Cursor extensions -- stays "node"
        assert_eq!(
            normalize_process_name(
                "/Applications/Cursor.app/Contents/Resources/app/resources/helpers/node"
            ),
            "node"
        );
        // Claude Code extension running inside Cursor -- stays "claude"
        assert_eq!(
            normalize_process_name(
                "/Users/rp/.cursor/extensions/anthropic.claude-code-2.1.79-darwin-arm64/resources/native-binary/claude"
            ),
            "claude"
        );
        // System CursorUIViewService -- NOT the editor, should stay separate
        assert_eq!(
            normalize_process_name("CursorUIViewService"),
            "CursorUIViewService"
        );
    }

    /// Verify that all Cursor variants group into a single "Cursor" group.
    #[test]
    fn test_build_groups_cursor_all_helpers_merge() {
        let processes = vec![
            ProcessInfo {
                pid: 100,
                cmd: "Cursor".into(),
                cpu_pct: 5.0,
                ram_gb: 0.5,
                blame_score: 0.1,
            },
            ProcessInfo {
                pid: 101,
                cmd: "Cursor Helper (Renderer)".into(),
                cpu_pct: 10.0,
                ram_gb: 0.3,
                blame_score: 0.2,
            },
            ProcessInfo {
                pid: 102,
                cmd: "Cursor Helper (Renderer)".into(),
                cpu_pct: 8.0,
                ram_gb: 0.2,
                blame_score: 0.15,
            },
            ProcessInfo {
                pid: 103,
                cmd: "Cursor Helper (GPU)".into(),
                cpu_pct: 3.0,
                ram_gb: 0.1,
                blame_score: 0.05,
            },
            ProcessInfo {
                pid: 104,
                cmd: "Cursor Helper (Plugin)".into(),
                cpu_pct: 2.0,
                ram_gb: 0.1,
                blame_score: 0.03,
            },
            ProcessInfo {
                pid: 105,
                cmd: "Cursor Helper (Plugin)".into(),
                cpu_pct: 1.0,
                ram_gb: 0.05,
                blame_score: 0.02,
            },
            ProcessInfo {
                pid: 106,
                cmd: "Cursor Helper".into(),
                cpu_pct: 1.0,
                ram_gb: 0.05,
                blame_score: 0.01,
            },
            // Node spawned by Cursor -- should be separate group
            ProcessInfo {
                pid: 200,
                cmd: "node".into(),
                cpu_pct: 5.0,
                ram_gb: 0.2,
                blame_score: 0.1,
            },
        ];
        let groups = build_groups(&processes);

        // Should have exactly 2 groups: "Cursor" and "node"
        assert_eq!(
            groups.len(),
            2,
            "groups: {:?}",
            groups.iter().map(|g| &g.name).collect::<Vec<_>>()
        );

        let cursor_group = groups
            .iter()
            .find(|g| g.name == "Cursor")
            .expect("Cursor group missing");
        assert_eq!(
            cursor_group.process_count, 7,
            "Cursor should have 7 processes"
        );
        assert_eq!(cursor_group.pids.len(), 7);
        assert!((cursor_group.total_cpu_pct - 30.0).abs() < 0.01);
        assert!((cursor_group.total_ram_gb - 1.3).abs() < 0.01);

        let node_group = groups
            .iter()
            .find(|g| g.name == "node")
            .expect("node group missing");
        assert_eq!(node_group.process_count, 1);
    }

    #[test]
    fn test_build_groups_aggregates_correctly() {
        let processes = vec![
            ProcessInfo {
                pid: 1,
                cmd: "Google Chrome Helper (GPU)".to_string(),
                cpu_pct: 10.0,
                ram_gb: 1.0,
                blame_score: 0.3,
            },
            ProcessInfo {
                pid: 2,
                cmd: "Google Chrome Helper (Renderer)".to_string(),
                cpu_pct: 20.0,
                ram_gb: 2.0,
                blame_score: 0.5,
            },
            ProcessInfo {
                pid: 3,
                cmd: "Google Chrome Helper".to_string(),
                cpu_pct: 5.0,
                ram_gb: 0.5,
                blame_score: 0.1,
            },
        ];

        let groups = build_groups(&processes);
        assert_eq!(groups.len(), 1);
        let g = &groups[0];
        assert_eq!(g.name, "Google Chrome");
        assert_eq!(g.process_count, 3);
        assert!((g.total_cpu_pct - 35.0).abs() < 0.01);
        assert!((g.total_ram_gb - 3.5).abs() < 0.01);
        assert!((g.blame_score - 0.5).abs() < 0.01);
        assert_eq!(g.top_pid, 2);
        assert_eq!(g.pids.len(), 3);
    }

    #[test]
    fn test_build_groups_separates_different_apps() {
        let processes = vec![
            ProcessInfo {
                pid: 1,
                cmd: "Google Chrome Helper".to_string(),
                cpu_pct: 10.0,
                ram_gb: 1.0,
                blame_score: 0.3,
            },
            ProcessInfo {
                pid: 2,
                cmd: "node".to_string(),
                cpu_pct: 50.0,
                ram_gb: 0.5,
                blame_score: 0.7,
            },
        ];

        let groups = build_groups(&processes);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].name, "node"); // higher blame
        assert_eq!(groups[1].name, "Google Chrome");
    }

    #[test]
    fn test_build_groups_sorted_by_blame() {
        let processes = vec![
            ProcessInfo {
                pid: 1,
                cmd: "chrome".to_string(),
                cpu_pct: 10.0,
                ram_gb: 1.0,
                blame_score: 0.2,
            },
            ProcessInfo {
                pid: 2,
                cmd: "node".to_string(),
                cpu_pct: 80.0,
                ram_gb: 0.5,
                blame_score: 0.9,
            },
            ProcessInfo {
                pid: 3,
                cmd: "cargo".to_string(),
                cpu_pct: 40.0,
                ram_gb: 2.0,
                blame_score: 0.5,
            },
        ];

        let groups = build_groups(&processes);
        assert_eq!(groups[0].name, "node");
        assert_eq!(groups[1].name, "cargo");
        assert_eq!(groups[2].name, "chrome");
    }

    #[test]
    fn test_single_process_group() {
        let processes = vec![ProcessInfo {
            pid: 42,
            cmd: "cargo".to_string(),
            cpu_pct: 80.0,
            ram_gb: 2.0,
            blame_score: 0.7,
        }];

        let groups = build_groups(&processes);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].process_count, 1);
        assert_eq!(groups[0].top_pid, 42);
    }

    #[test]
    fn test_empty_processes() {
        let groups = build_groups(&[]);
        assert!(groups.is_empty());
    }

    // ── Cmdline parsing tests ─────────────────────────────────────────────────

    fn null_join(args: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for arg in args {
            out.extend_from_slice(arg.as_bytes());
            out.push(0);
        }
        out
    }

    #[test]
    fn test_parse_cmdline_orchestrator_init_flag() {
        let raw = null_join(&[
            "/opt/claude-code/bin/claude",
            "--output-format=stream-json",
            "--init",
            "--session",
            "cse_abc123",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator, "should detect --init as orchestrator");
        assert_eq!(meta.session_id.as_deref(), Some("cse_abc123"));
    }

    #[test]
    fn test_parse_cmdline_orchestrator_resume_flag() {
        let raw = null_join(&[
            "claude",
            "--replay-user-messages",
            "--resume=https://api.example.com/sessions/abc",
            "--session",
            "cse_xyz789",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator, "--resume= marks orchestrator");
        assert_eq!(meta.session_id.as_deref(), Some("cse_xyz789"));
    }

    #[test]
    fn test_parse_cmdline_sub_agent_no_init() {
        let raw = null_join(&[
            "/opt/claude-code/bin/claude",
            "--output-format=stream-json",
            "--session",
            "cse_subagent456",
            "--model",
            "claude-sonnet-4-6",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(!meta.is_orchestrator, "sub-agent has no --init or --resume");
        assert_eq!(meta.session_id.as_deref(), Some("cse_subagent456"));
    }

    #[test]
    fn test_parse_cmdline_session_equals_form() {
        let raw = null_join(&["claude", "--session=cse_eq_form"]);
        let meta = parse_claude_cmdline(&raw);
        assert_eq!(meta.session_id.as_deref(), Some("cse_eq_form"));
    }

    #[test]
    fn test_parse_cmdline_no_session() {
        let raw = null_join(&["claude", "--help"]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.session_id.is_none());
        assert!(!meta.is_orchestrator);
    }

    #[test]
    fn test_parse_cmdline_replay_user_messages_is_orchestrator() {
        let raw = null_join(&["claude", "--replay-user-messages", "--session", "s1"]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator);
    }

    #[test]
    fn test_parse_cmdline_session_from_sdk_url() {
        let raw = null_join(&[
            "claude",
            "--init",
            "--sdk-url",
            "https://api.anthropic.com/v1/code/sessions/cse_011r3prb9N2MbmUbvbGg1qif",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator);
        assert_eq!(
            meta.session_id.as_deref(),
            Some("cse_011r3prb9N2MbmUbvbGg1qif")
        );
    }

    #[test]
    fn test_parse_cmdline_session_from_resume_url() {
        let raw = null_join(&[
            "claude",
            "--replay-user-messages",
            "--resume=https://api.anthropic.com/v1/code/sessions/cse_abc999def",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator);
        assert_eq!(meta.session_id.as_deref(), Some("cse_abc999def"));
    }

    #[test]
    fn test_parse_cmdline_session_from_mcp_config() {
        let raw = null_join(&[
            "claude",
            "--init",
            "--mcp-config",
            "/tmp/mcp-config-cse_mymcpsession.json",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert!(meta.is_orchestrator);
        assert_eq!(meta.session_id.as_deref(), Some("cse_mymcpsession"));
    }

    #[test]
    fn test_parse_cmdline_explicit_session_takes_priority_over_url() {
        // --session explicit value should win over URL-embedded one
        let raw = null_join(&[
            "claude",
            "--init",
            "--session",
            "cse_explicit",
            "--sdk-url",
            "https://api.anthropic.com/v1/code/sessions/cse_url_embedded",
        ]);
        let meta = parse_claude_cmdline(&raw);
        assert_eq!(meta.session_id.as_deref(), Some("cse_explicit"));
    }

    #[test]
    fn test_extract_session_from_url_helper() {
        assert_eq!(
            extract_session_from_url("https://api.anthropic.com/v1/code/sessions/cse_test123"),
            Some("cse_test123".to_string())
        );
        assert_eq!(
            extract_session_from_url("mcp-config-cse_filetest.json"),
            Some("cse_filetest".to_string())
        );
        assert_eq!(extract_session_from_url("--no-session-here"), None);
    }
}
