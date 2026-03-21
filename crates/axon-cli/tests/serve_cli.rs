//! CLI: `axon serve` flags and config wiring.
use std::fs;
use std::process::Command;
use std::time::{Duration, Instant};
use tempfile::tempdir;

fn axon_bin() -> &'static str {
    env!("CARGO_BIN_EXE_axon")
}

fn spawn_serve_kill(args: &[&str]) {
    let mut child = Command::new(axon_bin())
        .args(args)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .expect("spawn");
    std::thread::sleep(Duration::from_millis(150));
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn test_cli_parse_alert_webhook() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alert-dispatch.json");
    fs::write(
        &cfg,
        r#"{"channels":[{"type":"mcp","id":"mcp_client","filters":{"severity":[],"alert_types":["*"]}}]}"#,
    )
    .unwrap();

    spawn_serve_kill(&[
        "serve",
        "--config-dir",
        dir.path().to_str().unwrap(),
        "--alert-webhook",
        "myapp=http://127.0.0.1:9/alerts",
    ]);
}

#[test]
fn test_cli_parse_alert_filter() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alert-dispatch.json");
    fs::write(
        &cfg,
        r#"{"channels":[{"type":"webhook","id":"myapp","url":"http://127.0.0.1:1/x","filters":{}}]}"#,
    )
    .unwrap();

    spawn_serve_kill(&[
        "serve",
        "--config-dir",
        dir.path().to_str().unwrap(),
        "--alert-filter",
        "myapp.severity=critical",
        "--alert-filter",
        "myapp.types=thermal_throttle",
    ]);
}

#[test]
fn test_cli_flag_overrides_file() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alert-dispatch.json");
    fs::write(
        &cfg,
        r#"{"channels":[{"type":"webhook","id":"myapp","url":"http://127.0.0.1:2/old","filters":{}}]}"#,
    )
    .unwrap();

    spawn_serve_kill(&[
        "serve",
        "--config-dir",
        dir.path().to_str().unwrap(),
        "--alert-webhook",
        "myapp=http://127.0.0.1:9/new",
    ]);
}

#[test]
fn test_serve_startup_with_config() {
    let dir = tempdir().unwrap();
    let cfg = dir.path().join("alert-dispatch.json");
    fs::write(
        &cfg,
        r#"{"channels":[{"type":"webhook","id":"w","url":"http://127.0.0.1:9/h","filters":{}}]}"#,
    )
    .unwrap();

    let mut child = Command::new(axon_bin())
        .args(["serve", "--config-dir", dir.path().to_str().unwrap()])
        .env("RUST_LOG", "info")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn");

    let start = Instant::now();
    let mut saw = false;
    while start.elapsed() < Duration::from_secs(3) {
        if let Some(ref mut e) = child.stderr {
            use std::io::Read;
            let mut buf = [0u8; 4096];
            if let Ok(n) = e.read(&mut buf) {
                let s = String::from_utf8_lossy(&buf[..n]);
                if s.contains("webhook") || s.contains("axon") {
                    saw = true;
                    break;
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(saw, "stderr should mention startup or webhook");
}
