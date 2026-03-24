//! Integration tests for setup, setup --list, and uninstall commands.
//!
//! These tests use temp directories to simulate agent config files so they
//! run safely in CI without touching real agent configs.
//!
//! On Windows, `dirs::home_dir()` uses WinAPI (SHGetKnownFolderPath) which
//! ignores HOME/USERPROFILE env overrides, so these tests are skipped there.
//! The underlying JSON config logic is still validated by the unit tests and
//! by macOS/Linux CI.
#![cfg(not(target_os = "windows"))]

use std::process::Command;
use tempfile::tempdir;

fn axon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_axon")
}

// ── setup / uninstall with real agent configs (isolated via HOME override) ────

/// Create a fake HOME with the directory structure agents expect,
/// run a closure, then clean up.
fn with_fake_home(f: impl FnOnce(&std::path::Path)) {
    let dir = tempdir().unwrap();
    let home = dir.path();

    // Create dirs that axon setup checks for existence (platform-aware)
    #[cfg(target_os = "macos")]
    {
        std::fs::create_dir_all(home.join("Library/Application Support/Claude")).unwrap();
        std::fs::create_dir_all(home.join("Library/Application Support/Code/User")).unwrap();
        std::fs::write(
            home.join("Library/Application Support/Code/User/settings.json"),
            "{}",
        )
        .unwrap();
    }
    #[cfg(target_os = "linux")]
    {
        std::fs::create_dir_all(home.join(".config/Claude")).unwrap();
        std::fs::create_dir_all(home.join(".config/Code/User")).unwrap();
        std::fs::write(home.join(".config/Code/User/settings.json"), "{}").unwrap();
    }
    #[cfg(target_os = "windows")]
    {
        std::fs::create_dir_all(home.join("AppData/Roaming/Claude")).unwrap();
        std::fs::create_dir_all(home.join("AppData/Roaming/Code/User")).unwrap();
        std::fs::write(
            home.join("AppData/Roaming/Code/User/settings.json"),
            "{}",
        )
        .unwrap();
    }

    std::fs::create_dir_all(home.join(".cursor")).unwrap();

    f(home);
}

fn run_axon(home: &std::path::Path, args: &[&str]) -> (bool, String, String) {
    let mut cmd = Command::new(axon_bin());
    cmd.args(args).env("HOME", home);
    // On Windows, many libraries check USERPROFILE instead of HOME
    #[cfg(target_os = "windows")]
    cmd.env("USERPROFILE", home);
    let output = cmd.output().expect("failed to run axon");
    (
        output.status.success(),
        String::from_utf8_lossy(&output.stdout).to_string(),
        String::from_utf8_lossy(&output.stderr).to_string(),
    )
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[test]
fn test_setup_configures_agents() {
    with_fake_home(|home| {
        let (ok, stdout, _) = run_axon(home, &["setup"]);
        assert!(ok, "setup should succeed");
        assert!(stdout.contains("[ok] Configured Claude Desktop"));
        assert!(stdout.contains("[ok] Configured Cursor"));
        assert!(stdout.contains("[ok] Configured VS Code"));

        // Verify JSON was written correctly (platform-aware paths)
        #[cfg(target_os = "macos")]
        let claude_path =
            home.join("Library/Application Support/Claude/claude_desktop_config.json");
        #[cfg(target_os = "linux")]
        let claude_path = home.join(".config/Claude/claude_desktop_config.json");
        #[cfg(target_os = "windows")]
        let claude_path = home.join("AppData/Roaming/Claude/claude_desktop_config.json");

        let claude: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert!(claude["mcpServers"]["axon"]["command"].is_string());
        assert_eq!(claude["mcpServers"]["axon"]["args"][0], "serve");

        let cursor: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(home.join(".cursor/mcp.json")).unwrap())
                .unwrap();
        assert!(cursor["mcpServers"]["axon"]["command"].is_string());

        #[cfg(target_os = "macos")]
        let vscode_path = home.join("Library/Application Support/Code/User/settings.json");
        #[cfg(target_os = "linux")]
        let vscode_path = home.join(".config/Code/User/settings.json");
        #[cfg(target_os = "windows")]
        let vscode_path = home.join("AppData/Roaming/Code/User/settings.json");

        let vscode: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&vscode_path).unwrap()).unwrap();
    });
}

#[test]
fn test_setup_idempotent() {
    with_fake_home(|home| {
        run_axon(home, &["setup"]);
        let (ok, stdout, _) = run_axon(home, &["setup"]);
        assert!(ok);
        assert!(stdout.contains("already configured"));
    });
}

#[test]
fn test_setup_list_shows_status() {
    with_fake_home(|home| {
        // Before setup
        let (ok, stdout, _) = run_axon(home, &["setup", "--list"]);
        assert!(ok);
        assert!(stdout.contains("[--]"));
        assert!(stdout.contains("0 of"));

        // After setup
        run_axon(home, &["setup"]);
        let (ok, stdout, _) = run_axon(home, &["setup", "--list"]);
        assert!(ok);
        assert!(stdout.contains("[ok]"));
        assert!(stdout.contains("binary:"));
        assert!(stdout.contains("3 of"));
    });
}

#[test]
fn test_setup_single_target() {
    with_fake_home(|home| {
        let (ok, stdout, _) = run_axon(home, &["setup", "cursor"]);
        assert!(ok);
        assert!(stdout.contains("[ok] Updated"));

        // Only cursor should be configured
        let (_, stdout, _) = run_axon(home, &["setup", "--list"]);
        assert!(stdout.contains("1 of"));
    });
}

#[test]
fn test_uninstall_removes_from_all() {
    with_fake_home(|home| {
        run_axon(home, &["setup"]);
        let (ok, stdout, _) = run_axon(home, &["uninstall"]);
        assert!(ok);
        assert!(stdout.contains("Removed axon from Claude Desktop"));
        assert!(stdout.contains("Removed axon from Cursor"));
        assert!(stdout.contains("Removed axon from VS Code"));
        assert!(stdout.contains("Removed from 3 agent(s)"));

        // Verify configs no longer have axon
        #[cfg(target_os = "macos")]
        let claude_path =
            home.join("Library/Application Support/Claude/claude_desktop_config.json");
        #[cfg(target_os = "linux")]
        let claude_path = home.join(".config/Claude/claude_desktop_config.json");
        #[cfg(target_os = "windows")]
        let claude_path = home.join("AppData/Roaming/Claude/claude_desktop_config.json");

        let claude: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&claude_path).unwrap()).unwrap();
        assert!(claude["mcpServers"]["axon"].is_null());

        // setup --list should show 0
        let (_, stdout, _) = run_axon(home, &["setup", "--list"]);
        assert!(stdout.contains("0 of"));
    });
}

#[test]
fn test_uninstall_single_target() {
    with_fake_home(|home| {
        run_axon(home, &["setup"]);
        let (ok, stdout, _) = run_axon(home, &["uninstall", "cursor"]);
        assert!(ok);
        assert!(stdout.contains("Removed axon from Cursor"));

        // Others should still be configured
        let (_, stdout, _) = run_axon(home, &["setup", "--list"]);
        assert!(stdout.contains("2 of"));
    });
}

#[test]
fn test_uninstall_idempotent() {
    with_fake_home(|home| {
        let (ok, stdout, _) = run_axon(home, &["uninstall", "cursor"]);
        assert!(ok);
        assert!(stdout.contains("not configured"));
    });
}

#[test]
fn test_uninstall_unknown_target() {
    let output = Command::new(axon_bin())
        .args(["uninstall", "foobar"])
        .output()
        .expect("failed to run");
    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(stderr.contains("Unknown target"));
}

#[test]
fn test_uninstall_purges_data_dirs() {
    with_fake_home(|home| {
        // Create fake data dirs that uninstall should remove.
        // Use the platform-appropriate data directory path.
        #[cfg(target_os = "macos")]
        let data_dir = home.join("Library/Application Support/axon");
        #[cfg(target_os = "linux")]
        let data_dir = home.join(".local/share/axon");
        #[cfg(target_os = "windows")]
        let data_dir = home.join("AppData/Local/axon");

        std::fs::create_dir_all(&data_dir).unwrap();
        std::fs::write(data_dir.join("hardware.db"), "fake").unwrap();

        #[cfg(target_os = "windows")]
        let config_dir = home.join("AppData/Roaming/axon");
        #[cfg(not(target_os = "windows"))]
        let config_dir = home.join(".config/axon");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("alert-dispatch.json"), "{}").unwrap();

        let (ok, stdout, _) = run_axon(home, &["uninstall"]);
        assert!(ok);
        assert!(stdout.contains("Purged"));

        assert!(!data_dir.exists(), "data dir should be removed");
        assert!(!config_dir.exists(), "config dir should be removed");
    });
}

#[test]
fn test_setup_preserves_existing_config() {
    with_fake_home(|home| {
        // Write a cursor config with another MCP server
        let cursor_path = home.join(".cursor/mcp.json");
        std::fs::write(
            &cursor_path,
            r#"{"mcpServers":{"other-tool":{"command":"other","args":[]}}}"#,
        )
        .unwrap();

        run_axon(home, &["setup", "cursor"]);

        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cursor_path).unwrap()).unwrap();
        assert!(
            config["mcpServers"]["other-tool"].is_object(),
            "other-tool should be preserved"
        );
        assert!(
            config["mcpServers"]["axon"].is_object(),
            "axon should be added"
        );

        // Uninstall should only remove axon
        run_axon(home, &["uninstall", "cursor"]);
        let config: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cursor_path).unwrap()).unwrap();
        assert!(
            config["mcpServers"]["other-tool"].is_object(),
            "other-tool should still be there"
        );
        assert!(
            config["mcpServers"]["axon"].is_null(),
            "axon should be removed"
        );
    });
}
