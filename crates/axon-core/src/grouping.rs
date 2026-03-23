use std::collections::HashMap;

use crate::types::{ProcessGroup, ProcessInfo};

/// Normalize a process name to its app-level group name.
/// Strips helper suffixes, path prefixes, and maps known executables to display names.
pub fn normalize_process_name(cmd: &str) -> String {
    let name = cmd.trim_end_matches('\0').trim();

    // Strip path prefix (e.g., "/usr/bin/node" -> "node")
    let base = name.split('/').next_back().unwrap_or(name);

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
            normalize_process_name(
                "/Applications/Cursor.app/Contents/MacOS/Cursor"
            ),
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
            ProcessInfo { pid: 100, cmd: "Cursor".into(), cpu_pct: 5.0, ram_gb: 0.5, blame_score: 0.1 },
            ProcessInfo { pid: 101, cmd: "Cursor Helper (Renderer)".into(), cpu_pct: 10.0, ram_gb: 0.3, blame_score: 0.2 },
            ProcessInfo { pid: 102, cmd: "Cursor Helper (Renderer)".into(), cpu_pct: 8.0, ram_gb: 0.2, blame_score: 0.15 },
            ProcessInfo { pid: 103, cmd: "Cursor Helper (GPU)".into(), cpu_pct: 3.0, ram_gb: 0.1, blame_score: 0.05 },
            ProcessInfo { pid: 104, cmd: "Cursor Helper (Plugin)".into(), cpu_pct: 2.0, ram_gb: 0.1, blame_score: 0.03 },
            ProcessInfo { pid: 105, cmd: "Cursor Helper (Plugin)".into(), cpu_pct: 1.0, ram_gb: 0.05, blame_score: 0.02 },
            ProcessInfo { pid: 106, cmd: "Cursor Helper".into(), cpu_pct: 1.0, ram_gb: 0.05, blame_score: 0.01 },
            // Node spawned by Cursor -- should be separate group
            ProcessInfo { pid: 200, cmd: "node".into(), cpu_pct: 5.0, ram_gb: 0.2, blame_score: 0.1 },
        ];
        let groups = build_groups(&processes);

        // Should have exactly 2 groups: "Cursor" and "node"
        assert_eq!(groups.len(), 2, "groups: {:?}", groups.iter().map(|g| &g.name).collect::<Vec<_>>());

        let cursor_group = groups.iter().find(|g| g.name == "Cursor").expect("Cursor group missing");
        assert_eq!(cursor_group.process_count, 7, "Cursor should have 7 processes");
        assert_eq!(cursor_group.pids.len(), 7);
        assert!((cursor_group.total_cpu_pct - 30.0).abs() < 0.01);
        assert!((cursor_group.total_ram_gb - 1.3).abs() < 0.01);

        let node_group = groups.iter().find(|g| g.name == "node").expect("node group missing");
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
}
