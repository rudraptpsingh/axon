use std::collections::HashMap;

use crate::types::{ProcessGroup, ProcessInfo};

/// Normalize a process name to its app-level group name.
/// Strips helper suffixes, path prefixes, and maps known executables to display names.
pub fn normalize_process_name(cmd: &str) -> String {
    let name = cmd.trim_end_matches('\0').trim();

    // Strip path prefix (e.g., "/usr/bin/node" -> "node")
    let base = name.split('/').last().unwrap_or(name);

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
        let entry = map.entry(group_name.clone()).or_insert_with(|| ProcessGroup {
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
