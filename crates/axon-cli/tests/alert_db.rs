//! Integration tests for alert persistence in SQLite.
//!
//! These run against real hardware (need macOS collector) so they are #[ignore].
//! CI runs them in the `integration` job with `--ignored`.
use std::process::Command;
use tempfile::tempdir;

fn axon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_axon")
}

#[test]
#[ignore] // ~5s: needs real hardware
fn test_diagnose_creates_db_and_alerts() {
    let data_dir = tempdir().unwrap();

    let output = Command::new(axon_bin())
        .arg("diagnose")
        .env("AXON_DATA_DIR", data_dir.path())
        .output()
        .expect("failed to run axon diagnose");

    assert!(output.status.success(), "diagnose should succeed");

    let db_path = data_dir.path().join("hardware.db");
    assert!(db_path.exists(), "hardware.db should be created");

    // Check tables exist and have data
    let tables = Command::new("sqlite3")
        .args([db_path.to_str().unwrap(), ".tables"])
        .output()
        .expect("sqlite3");
    let tables_str = String::from_utf8_lossy(&tables.stdout);
    assert!(tables_str.contains("snapshots"), "snapshots table missing");
    assert!(tables_str.contains("alerts"), "alerts table missing");

    // Snapshots should have rows
    let count = Command::new("sqlite3")
        .args([db_path.to_str().unwrap(), "SELECT COUNT(*) FROM snapshots;"])
        .output()
        .expect("sqlite3");
    let n: i64 = String::from_utf8_lossy(&count.stdout)
        .trim()
        .parse()
        .unwrap_or(0);
    assert!(n > 0, "expected snapshots, got {}", n);
}

#[test]
#[ignore] // ~5s: needs real hardware
fn test_alert_schema_has_required_columns() {
    let data_dir = tempdir().unwrap();

    Command::new(axon_bin())
        .arg("diagnose")
        .env("AXON_DATA_DIR", data_dir.path())
        .output()
        .expect("failed to run axon diagnose");

    let db_path = data_dir.path().join("hardware.db");
    let schema = Command::new("sqlite3")
        .args([db_path.to_str().unwrap(), ".schema alerts"])
        .output()
        .expect("sqlite3");
    let schema_str = String::from_utf8_lossy(&schema.stdout);

    for col in &[
        "id",
        "ts",
        "severity",
        "alert_type",
        "message",
        "metadata_json",
    ] {
        assert!(
            schema_str.contains(col),
            "alerts schema missing column: {}",
            col
        );
    }
}

#[test]
#[ignore] // ~5s: needs real hardware
fn test_uninstall_purges_db() {
    let data_dir = tempdir().unwrap();
    let home_dir = tempdir().unwrap();
    let home = home_dir.path();

    // Create dirs for setup
    std::fs::create_dir_all(home.join(".cursor")).unwrap();

    // Run diagnose to create DB
    Command::new(axon_bin())
        .arg("diagnose")
        .env("AXON_DATA_DIR", data_dir.path())
        .output()
        .expect("diagnose");

    let db_path = data_dir.path().join("hardware.db");
    assert!(db_path.exists(), "DB should exist before uninstall");

    // Uninstall with HOME override (purge uses HOME-relative paths per platform)
    // Create the data dir where uninstall expects it
    #[cfg(target_os = "macos")]
    let axon_data = home.join("Library/Application Support/axon");
    #[cfg(target_os = "linux")]
    let axon_data = home.join(".local/share/axon");
    #[cfg(target_os = "windows")]
    let axon_data = home.join("AppData/Local/axon");
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let axon_data = home.join(".local/share/axon");
    std::fs::create_dir_all(&axon_data).unwrap();
    std::fs::write(axon_data.join("hardware.db"), "fake").unwrap();

    let output = Command::new(axon_bin())
        .args(["uninstall"])
        .env("HOME", home)
        .output()
        .expect("uninstall");

    assert!(output.status.success());
    assert!(
        !axon_data.exists(),
        "data dir should be removed after uninstall"
    );
}
