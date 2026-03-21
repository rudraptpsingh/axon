use std::io::{BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::time::Duration;

fn axon_bin() -> String {
    // Use cargo-built binary
    env!("CARGO_BIN_EXE_axon").to_string()
}

fn spawn_server() -> std::process::Child {
    Command::new(axon_bin())
        .arg("serve")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("Failed to start axon serve")
}

fn send_and_collect(messages: &[&str], wait_secs: u64) -> Vec<serde_json::Value> {
    let mut child = spawn_server();
    let mut stdin = child.stdin.take().expect("stdin");

    // Send all messages
    for msg in messages {
        writeln!(stdin, "{}", msg).expect("write to stdin");
    }

    // Wait for collector warm-up
    std::thread::sleep(Duration::from_secs(wait_secs));

    // Close stdin to signal EOF
    drop(stdin);

    // Read all stdout lines
    let stdout = child.stdout.take().expect("stdout");
    let reader = BufReader::new(stdout);
    let mut responses: Vec<serde_json::Value> = Vec::new();

    for line in reader.lines() {
        if let Ok(line) = line {
            if let Ok(val) = serde_json::from_str::<serde_json::Value>(&line) {
                responses.push(val);
            }
        }
    }

    let _ = child.wait();
    responses
}

const INIT_MSG: &str = r#"{"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"0.1.0"}},"jsonrpc":"2.0","id":0}"#;
const INITIALIZED_MSG: &str = r#"{"method":"notifications/initialized","jsonrpc":"2.0"}"#;

#[test]
#[ignore] // takes ~5s due to collector warm-up
fn test_initialize_response() {
    let responses = send_and_collect(&[INIT_MSG], 1);
    assert!(!responses.is_empty(), "expected at least one response");

    let resp = &responses[0];
    assert_eq!(resp["jsonrpc"], "2.0");
    assert_eq!(resp["id"], 0);
    assert!(
        resp["result"]["serverInfo"].is_object(),
        "missing serverInfo"
    );
    assert!(
        resp["result"]["capabilities"].is_object(),
        "missing capabilities"
    );
}

#[test]
#[ignore] // takes ~5s
fn test_tools_list() {
    let list_msg = r#"{"method":"tools/list","jsonrpc":"2.0","id":1}"#;
    let responses = send_and_collect(&[INIT_MSG, INITIALIZED_MSG, list_msg], 2);

    // Find the tools/list response (id=1)
    let resp = responses
        .iter()
        .find(|r| r["id"] == 1)
        .expect("no tools/list response");

    let tools = resp["result"]["tools"]
        .as_array()
        .expect("tools should be an array");
    assert_eq!(tools.len(), 5, "expected 5 tools, got {}", tools.len());

    let names: Vec<&str> = tools.iter().map(|t| t["name"].as_str().unwrap()).collect();
    assert!(names.contains(&"hw_snapshot"));
    assert!(names.contains(&"process_blame"));
    assert!(names.contains(&"battery_status"));
    assert!(names.contains(&"hardware_trend"));
    assert!(names.contains(&"system_profile"));

    // Each tool should have a description and inputSchema
    for tool in tools {
        assert!(tool["description"].is_string(), "tool missing description");
        assert!(tool["inputSchema"].is_object(), "tool missing inputSchema");
    }
}

#[test]
#[ignore] // takes ~6s (4s warm-up + call)
fn test_hw_snapshot_call() {
    let call_msg = r#"{"method":"tools/call","params":{"name":"hw_snapshot","arguments":{}},"jsonrpc":"2.0","id":2}"#;
    let responses = send_and_collect(&[INIT_MSG, INITIALIZED_MSG, call_msg], 5);

    let resp = responses
        .iter()
        .find(|r| r["id"] == 2)
        .expect("no hw_snapshot response");

    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("missing content text");
    let data: serde_json::Value = serde_json::from_str(text).expect("content is not valid JSON");

    assert_eq!(data["ok"], true);
    assert!(data["ts"].is_string(), "missing timestamp");
    assert!(data["narrative"].is_string(), "missing narrative");
    assert!(
        data["data"]["cpu_usage_pct"].is_number(),
        "missing cpu_usage_pct"
    );
    assert!(
        data["data"]["ram_total_gb"].is_number(),
        "missing ram_total_gb"
    );
    assert!(
        data["data"]["ram_pressure"].is_string(),
        "missing ram_pressure"
    );
}

#[test]
#[ignore] // takes ~6s
fn test_process_blame_call() {
    let call_msg = r#"{"method":"tools/call","params":{"name":"process_blame","arguments":{}},"jsonrpc":"2.0","id":3}"#;
    let responses = send_and_collect(&[INIT_MSG, INITIALIZED_MSG, call_msg], 5);

    let resp = responses
        .iter()
        .find(|r| r["id"] == 3)
        .expect("no process_blame response");

    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("missing content text");
    let data: serde_json::Value = serde_json::from_str(text).expect("content is not valid JSON");

    assert_eq!(data["ok"], true);
    assert!(data["data"]["anomaly_type"].is_string());
    assert!(data["data"]["impact_level"].is_string());
    assert!(data["data"]["anomaly_score"].is_number());

    let score = data["data"]["anomaly_score"].as_f64().unwrap();
    assert!(
        (0.0..=1.0).contains(&score),
        "anomaly_score out of range: {}",
        score
    );
}

#[test]
#[ignore] // takes ~6s
fn test_system_profile_call() {
    let call_msg = r#"{"method":"tools/call","params":{"name":"system_profile","arguments":{}},"jsonrpc":"2.0","id":4}"#;
    let responses = send_and_collect(&[INIT_MSG, INITIALIZED_MSG, call_msg], 5);

    let resp = responses
        .iter()
        .find(|r| r["id"] == 4)
        .expect("no system_profile response");

    let text = resp["result"]["content"][0]["text"]
        .as_str()
        .expect("missing content text");
    let data: serde_json::Value = serde_json::from_str(text).expect("content is not valid JSON");

    assert_eq!(data["ok"], true);
    assert!(data["data"]["core_count"].as_u64().unwrap() > 0);
    assert!(data["data"]["ram_total_gb"].as_f64().unwrap() > 0.0);
    assert!(data["data"]["os_version"].is_string());
    assert!(data["data"]["axon_version"].is_string());
}
