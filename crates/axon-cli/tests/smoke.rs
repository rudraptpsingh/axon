use std::process::Command;

fn axon_bin() -> String {
    env!("CARGO_BIN_EXE_axon").to_string()
}

#[test]
#[ignore] // takes ~5s (diagnose collects 4s of data)
fn test_diagnose_output_format() {
    let output = Command::new(axon_bin())
        .arg("diagnose")
        .output()
        .expect("failed to run axon diagnose");

    assert!(output.status.success(), "diagnose should exit 0");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("[info] Collecting system data"),
        "stderr should show collection message"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[ok]") || stdout.contains("[warn]"),
        "stdout should contain [ok] or [warn], got: {}",
        stdout
    );
}

#[test]
#[ignore] // takes ~3s (status collects 2s of data)
fn test_status_outputs_valid_json() {
    let output = Command::new(axon_bin())
        .arg("status")
        .output()
        .expect("failed to run axon status");

    assert!(output.status.success(), "status should exit 0");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&stdout).expect("status output should be valid JSON");

    assert!(json["cpu_usage_pct"].is_number());
    assert!(json["ram_used_gb"].is_number());
    assert!(json["ram_total_gb"].is_number());
}

#[test]
fn test_setup_unknown_target_fails() {
    let output = Command::new(axon_bin())
        .args(["setup", "garbage"])
        .output()
        .expect("failed to run axon setup");

    assert!(!output.status.success(), "unknown target should fail");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("Unknown target"),
        "stderr should mention unknown target, got: {}",
        stderr
    );
}

#[test]
fn test_version_flag() {
    let output = Command::new(axon_bin())
        .arg("--version")
        .output()
        .expect("failed to run axon --version");

    assert!(output.status.success());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("axon"),
        "version output should contain 'axon', got: {}",
        stdout
    );
}
