//! Agent loop integration tests.
//!
//! Simulates real AI agents receiving Axon alerts via MCP and acting on them.
//! Three agent strategies:
//!   - Reactive:   wait for MCP notification -> process_blame -> kill culprit -> verify
//!   - Proactive:  poll hw_snapshot periodically -> detect degradation -> process_blame -> kill -> verify
//!   - Monitoring:  collect all alerts + snapshots for a window, report summary
//!
//! Run:
//!   cargo test -p axon --test agent_loop -- --ignored --nocapture
//!
//! Each test runs ~40-60 seconds. Uses isolated config/data dirs.

#![cfg(unix)]

use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use tempfile::TempDir;

// ---------------------------------------------------------------------------
// MCP Client — handles both responses (by id) and notifications (no id)
// ---------------------------------------------------------------------------

struct McpClient {
    stdin: std::process::ChildStdin,
    responses: mpsc::Receiver<serde_json::Value>,
    notifications: mpsc::Receiver<serde_json::Value>,
    next_id: u64,
}

impl McpClient {
    /// Connect to an axon serve process. Sends MCP initialize handshake.
    /// Returns the client and the child process handle.
    fn connect(
        bin: &str,
        config_dir: &std::path::Path,
        data_dir: &std::path::Path,
    ) -> (Self, std::process::Child) {
        let mut child = Command::new(bin)
            .arg("serve")
            .env("AXON_CONFIG_DIR", config_dir)
            .env("AXON_DATA_DIR", data_dir)
            .env("AXON_TEST_PREV_RAM_PRESSURE", "normal")
            .env("AXON_TEST_PREV_IMPACT_LEVEL", "healthy")
            .env("AXON_TEST_PREV_THROTTLING", "0")
            .env("AXON_TEST_PRESERVE_PREV_DURING_WARMUP", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("spawn axon");

        let mut stdin = child.stdin.take().expect("stdin");
        let stdout = child.stdout.take().expect("stdout");

        // Background reader: routes responses (has "id") vs notifications (no "id")
        let (resp_tx, resp_rx) = mpsc::channel();
        let (notif_tx, notif_rx) = mpsc::channel();
        thread::spawn(move || {
            let reader = BufReader::new(stdout);
            for line in reader.lines() {
                let Ok(line) = line else { break };
                let Ok(msg) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                if msg.get("id").is_some() {
                    let _ = resp_tx.send(msg);
                } else {
                    let _ = notif_tx.send(msg);
                }
            }
        });

        // MCP handshake
        let init = serde_json::json!({
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "agent-loop-test", "version": "0.1.0"},
            }
        });
        writeln!(stdin, "{}", init).expect("write init");
        let notif = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/initialized"});
        writeln!(stdin, "{}", notif).expect("write notif");

        // Wait for init response
        let deadline = Instant::now() + Duration::from_secs(15);
        loop {
            if Instant::now() > deadline {
                panic!("MCP init timed out");
            }
            if let Ok(msg) = resp_rx.recv_timeout(Duration::from_millis(200)) {
                if msg.get("id") == Some(&serde_json::json!(0)) && msg.get("result").is_some() {
                    break;
                }
            }
        }

        let client = McpClient {
            stdin,
            responses: resp_rx,
            notifications: notif_rx,
            next_id: 1,
        };
        (client, child)
    }

    /// Call an MCP tool. Returns the parsed inner JSON (the text content).
    fn call_tool(&mut self, name: &str) -> Option<serde_json::Value> {
        self.call_tool_with_args(name, serde_json::json!({}))
    }

    fn call_tool_with_args(
        &mut self,
        name: &str,
        arguments: serde_json::Value,
    ) -> Option<serde_json::Value> {
        let id = self.next_id;
        self.next_id += 1;
        let req = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "method": "tools/call",
            "params": {"name": name, "arguments": arguments}
        });
        writeln!(self.stdin, "{}", req).ok()?;

        let deadline = Instant::now() + Duration::from_secs(30);
        while Instant::now() < deadline {
            if let Ok(msg) = self.responses.recv_timeout(Duration::from_millis(200)) {
                if msg.get("id") == Some(&serde_json::json!(id)) {
                    let text = msg
                        .get("result")
                        .and_then(|r| r.get("content"))
                        .and_then(|c| c.get(0))
                        .and_then(|c| c.get("text"))
                        .and_then(|t| t.as_str())?;
                    return serde_json::from_str(text).ok();
                }
            }
        }
        None
    }

    /// Wait for an MCP notification (notifications/message). Returns the notification params.
    fn wait_notification(&self, timeout: Duration) -> Option<serde_json::Value> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if let Ok(msg) = self.notifications.recv_timeout(Duration::from_millis(200)) {
                return Some(msg);
            }
        }
        None
    }

    /// Drain any queued notifications (e.g. warmup alerts).
    fn drain_notifications(&self) -> usize {
        let mut count = 0;
        while self.notifications.try_recv().is_ok() {
            count += 1;
        }
        count
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn read_http_post_body(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
    use std::io::Read;
    let mut buf = Vec::new();
    let mut tmp = [0u8; 4096];
    loop {
        if let Some(end) = find_headers_end(&buf) {
            let headers = std::str::from_utf8(&buf[..end])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let mut content_length = 0usize;
            for line in headers.lines().skip(1) {
                let lower = line.to_ascii_lowercase();
                if lower.starts_with("content-length:") {
                    content_length = line
                        .split(':')
                        .nth(1)
                        .and_then(|s| s.trim().parse().ok())
                        .unwrap_or(0);
                }
            }
            let body_start = end + 4;
            let need = body_start + content_length;
            while buf.len() < need {
                let n = stream.read(&mut tmp)?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "short body",
                    ));
                }
                buf.extend_from_slice(&tmp[..n]);
            }
            return String::from_utf8(buf[body_start..need].to_vec())
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()));
        }
        let n = stream.read(&mut tmp)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "headers",
            ));
        }
        buf.extend_from_slice(&tmp[..n]);
        if buf.len() > 2 * 1024 * 1024 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "request too large",
            ));
        }
    }
}

fn start_webhook_receiver() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    listener.set_nonblocking(false).expect("blocking listener");
    let port = listener.local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{}/alerts", port);
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let response = b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        loop {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
            if let Ok(body) = read_http_post_body(&mut stream) {
                let _ = tx.send(body);
            }
            let _ = std::io::Write::write_all(&mut stream, response);
        }
    });
    (url, rx)
}

fn spawn_cpu_stress() -> Vec<std::process::Child> {
    let ncpu = std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        .min(16);
    let count = ncpu * 2;
    let mut kids = Vec::new();
    for _ in 0..count {
        if let Ok(p) = Command::new("yes")
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            kids.push(p);
        }
    }
    for _ in 0..ncpu {
        if let Ok(p) = Command::new("dd")
            .args(["if=/dev/urandom", "of=/dev/null", "bs=1M", "count=99999"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
        {
            kids.push(p);
        }
    }
    kids
}

fn kill_all(kids: &mut Vec<std::process::Child>) {
    for p in kids.iter_mut() {
        let _ = p.kill();
        let _ = p.wait();
    }
    kids.clear();
}

fn kill_pids(pids: &[u32]) {
    for &pid in pids {
        let _ = Command::new("kill")
            .args(["-9", &pid.to_string()])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    // Brief wait for processes to exit
    thread::sleep(Duration::from_millis(500));
}

fn write_dispatch_config(config_dir: &std::path::Path, webhook_url: &str) {
    let dispatch = serde_json::json!({
        "channels": [{
            "type": "webhook",
            "id": "agent_test",
            "url": webhook_url,
            "filters": { "severity": [], "alert_types": ["*"] }
        }]
    });
    std::fs::write(
        config_dir.join("alert-dispatch.json"),
        serde_json::to_string_pretty(&dispatch).unwrap(),
    )
    .unwrap();
}

fn run_proxy_task() -> f64 {
    let start = Instant::now();
    let script_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("scripts")
        .join("perf_proxy_task.sh");
    let r = Command::new("bash")
        .arg(&script_dir)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output();
    match r {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout)
            .trim()
            .parse::<f64>()
            .unwrap_or(start.elapsed().as_secs_f64()),
        _ => start.elapsed().as_secs_f64(),
    }
}

fn drain_webhook_alerts(rx: &mpsc::Receiver<String>) -> usize {
    let mut count = 0;
    while rx.try_recv().is_ok() {
        count += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// Reactive agent: waits for MCP notification, calls process_blame, kills culprit by PID,
/// verifies recovery via hw_snapshot. This is what Claude Code / Cursor would do.
#[test]
#[ignore = "live hardware; ~60s; run with: cargo test -p axon --test agent_loop -- --ignored --nocapture"]
fn agent_reactive() {
    let bin = env!("CARGO_BIN_EXE_axon");
    eprintln!("\n=== REACTIVE AGENT (Claude Code / Cursor style) ===\n");

    // Baseline
    let baseline = run_proxy_task();
    eprintln!("[agent] baseline task: {:.2}s", baseline);

    // Setup
    let config_dir = TempDir::new().expect("tmpdir");
    let data_dir = TempDir::new().expect("tmpdir");
    let (webhook_url, rx_webhook) = start_webhook_receiver();
    write_dispatch_config(config_dir.path(), &webhook_url);

    let (mut mcp, mut axon) = McpClient::connect(bin, config_dir.path(), data_dir.path());
    eprintln!("[agent] MCP connected");

    // Warmup
    eprintln!("[agent] warming up collector (12s)...");
    thread::sleep(Duration::from_secs(12));
    let drained = mcp.drain_notifications();
    drain_webhook_alerts(&rx_webhook);
    eprintln!("[agent] drained {} warmup notification(s)", drained);

    // Start stress
    eprintln!("[agent] starting CPU stress...");
    let mut stress = spawn_cpu_stress();
    let stress_start = Instant::now();

    // Step 1: Wait for MCP notification (the trigger)
    eprintln!("[agent] waiting for MCP alert notification...");
    let mut mttd: Option<Duration> = None;
    let timeout = Duration::from_secs(60);
    loop {
        if stress_start.elapsed() > timeout {
            eprintln!("[agent] no MCP notification in 60s, falling back to webhook...");
            // Fallback: check webhook
            if let Ok(_body) = rx_webhook.recv_timeout(Duration::from_millis(100)) {
                mttd = Some(stress_start.elapsed());
            }
            break;
        }
        if let Some(notif) = mcp.wait_notification(Duration::from_millis(500)) {
            mttd = Some(stress_start.elapsed());
            let alert_msg = notif
                .get("params")
                .and_then(|p| p.get("data"))
                .and_then(|d| d.as_str())
                .unwrap_or("");
            let level = notif
                .get("params")
                .and_then(|p| p.get("level"))
                .and_then(|l| l.as_str())
                .unwrap_or("unknown");
            eprintln!(
                "[agent] ALERT at +{:.1}s: level={} msg={:?}",
                mttd.unwrap().as_secs_f64(),
                level,
                &alert_msg[..alert_msg.len().min(80)]
            );
            break;
        }
    }

    // Step 2: Agent calls process_blame to diagnose
    eprintln!("[agent] calling process_blame...");
    let blame_start = Instant::now();
    let blame = mcp.call_tool("process_blame");
    let diagnose_time = blame_start.elapsed();
    eprintln!("[agent] diagnosis took {:.2}s", diagnose_time.as_secs_f64());

    let (culprit_name, culprit_pids, fix_suggestion, _impact_level, blame_correct) =
        if let Some(ref blame_data) = blame {
            let data = blame_data.get("data").unwrap_or(blame_data);
            let name = data
                .get("culprit_group")
                .and_then(|g| g.get("name"))
                .or_else(|| data.get("culprit").and_then(|c| c.get("cmd")))
                .and_then(|n| n.as_str())
                .unwrap_or("unknown")
                .to_string();
            let level = data
                .get("impact_level")
                .and_then(|l| l.as_str())
                .unwrap_or("unknown")
                .to_string();
            let fix = data
                .get("fix")
                .and_then(|f| f.as_str())
                .unwrap_or("")
                .to_string();
            let pids: Vec<u32> = data
                .get("culprit_group")
                .and_then(|g| g.get("pids"))
                .and_then(|p| p.as_array())
                .map(|arr| {
                    arr.iter()
                        .filter_map(|v| v.as_u64().map(|n| n as u32))
                        .collect()
                })
                .unwrap_or_default();
            let correct = name == "yes"
                || name == "dd"
                || name.contains("yes")
                || name.contains("dd");

            eprintln!("[agent] culprit: {} ({} PIDs)", name, pids.len());
            eprintln!("[agent] impact:  {}", level);
            eprintln!("[agent] fix:     {:?}", fix);
            eprintln!("[agent] blame correct: {}", correct);
            (name, pids, fix, level, correct)
        } else {
            eprintln!("[agent] process_blame returned no data");
            (String::new(), Vec::new(), String::new(), String::new(), false)
        };

    // Step 3: Agent applies the fix — kill the culprit PIDs
    eprintln!("[agent] applying fix: killing culprit PIDs...");
    let fix_start = Instant::now();
    if !culprit_pids.is_empty() {
        kill_pids(&culprit_pids);
    }
    // Also kill our stress handles (some PIDs might not match if grouping differs)
    kill_all(&mut stress);
    let fix_time = fix_start.elapsed();
    eprintln!("[agent] fix applied in {:.2}s", fix_time.as_secs_f64());

    // Step 4: Agent verifies recovery via hw_snapshot
    eprintln!("[agent] waiting 5s for system recovery...");
    thread::sleep(Duration::from_secs(5));

    let snapshot = mcp.call_tool("hw_snapshot");
    let post_fix_cpu = snapshot
        .as_ref()
        .and_then(|s| s.get("data"))
        .and_then(|d| d.get("cpu_usage_pct"))
        .and_then(|c| c.as_f64())
        .unwrap_or(-1.0);
    eprintln!("[agent] post-fix CPU: {:.1}%", post_fix_cpu);

    let recovered_time = run_proxy_task();
    let recovery_factor = recovered_time / baseline;
    eprintln!(
        "[agent] recovered task: {:.2}s ({:.1}x baseline)",
        recovered_time, recovery_factor
    );

    let mttr = mttd
        .map(|mttd_d| (stress_start.elapsed() - mttd_d).as_secs_f64())
        .unwrap_or(-1.0);

    // Cleanup
    let _ = axon.kill();
    let _ = axon.wait();

    // Results
    eprintln!("\n[agent] === REACTIVE AGENT RESULTS ===");
    eprintln!("[agent] Baseline:     {:.2}s", baseline);
    if let Some(d) = mttd {
        eprintln!("[agent] MTTD:         {:.1}s", d.as_secs_f64());
    } else {
        eprintln!("[agent] MTTD:         no alert");
    }
    eprintln!("[agent] Diagnosis:    {:.2}s", diagnose_time.as_secs_f64());
    eprintln!("[agent] Culprit:      {}", culprit_name);
    eprintln!("[agent] Blame correct: {}", blame_correct);
    eprintln!("[agent] Fix:          {:?}", fix_suggestion);
    eprintln!("[agent] MTTR:         {:.1}s", mttr);
    eprintln!("[agent] Recovery:     {:.2}s ({:.1}x)", recovered_time, recovery_factor);

    // Assertions
    assert!(
        recovery_factor < 2.0,
        "Recovery too slow: {:.1}x (target <2.0x)",
        recovery_factor
    );
    assert!(blame.is_some(), "process_blame should return data");
    assert!(blame_correct, "blame should identify stress processes, got: {}", culprit_name);
}

/// Proactive agent: polls hw_snapshot every few seconds, detects degradation without
/// waiting for a notification, then diagnoses and fixes. This is what a smarter agent
/// would do when MCP notifications aren't surfaced by the client.
#[test]
#[ignore = "live hardware; ~60s; run with: cargo test -p axon --test agent_loop -- --ignored --nocapture"]
fn agent_proactive() {
    let bin = env!("CARGO_BIN_EXE_axon");
    eprintln!("\n=== PROACTIVE AGENT (polling hw_snapshot) ===\n");

    let baseline = run_proxy_task();
    eprintln!("[agent] baseline task: {:.2}s", baseline);

    let config_dir = TempDir::new().expect("tmpdir");
    let data_dir = TempDir::new().expect("tmpdir");
    let (webhook_url, _rx_webhook) = start_webhook_receiver();
    write_dispatch_config(config_dir.path(), &webhook_url);

    let (mut mcp, mut axon) = McpClient::connect(bin, config_dir.path(), data_dir.path());
    eprintln!("[agent] MCP connected");

    // Warmup
    eprintln!("[agent] warming up collector (12s)...");
    thread::sleep(Duration::from_secs(12));
    mcp.drain_notifications();

    // Take baseline snapshot
    let baseline_snap = mcp.call_tool("hw_snapshot");
    let baseline_cpu = baseline_snap
        .as_ref()
        .and_then(|s| s.get("data"))
        .and_then(|d| d.get("cpu_usage_pct"))
        .and_then(|c| c.as_f64())
        .unwrap_or(0.0);
    eprintln!("[agent] baseline CPU from hw_snapshot: {:.1}%", baseline_cpu);

    // Start stress
    eprintln!("[agent] starting CPU stress...");
    let mut stress = spawn_cpu_stress();
    let stress_start = Instant::now();

    // Step 1: Poll hw_snapshot until we detect degradation
    eprintln!("[agent] polling hw_snapshot for degradation...");
    let mut detect_time: Option<Duration> = None;
    let mut detected_cpu = 0.0;
    let poll_interval = Duration::from_secs(3);
    let timeout = Duration::from_secs(60);

    loop {
        if stress_start.elapsed() > timeout {
            eprintln!("[agent] timeout: no degradation detected via polling");
            break;
        }
        thread::sleep(poll_interval);

        if let Some(snap) = mcp.call_tool("hw_snapshot") {
            let cpu = snap
                .get("data")
                .and_then(|d| d.get("cpu_usage_pct"))
                .and_then(|c| c.as_f64())
                .unwrap_or(0.0);
            let ram_pressure = snap
                .get("data")
                .and_then(|d| d.get("ram_pressure"))
                .and_then(|r| r.as_str())
                .unwrap_or("unknown");
            eprintln!(
                "[agent] poll +{:.0}s: CPU {:.1}%, RAM pressure: {}",
                stress_start.elapsed().as_secs_f64(),
                cpu,
                ram_pressure
            );

            // Detect: CPU jumped significantly or RAM pressure elevated
            if cpu > baseline_cpu + 20.0 || ram_pressure != "normal" {
                detect_time = Some(stress_start.elapsed());
                detected_cpu = cpu;
                eprintln!(
                    "[agent] DEGRADATION at +{:.1}s: CPU {:.1}% (baseline {:.1}%)",
                    detect_time.unwrap().as_secs_f64(),
                    cpu,
                    baseline_cpu
                );
                break;
            }
        }
    }

    // Step 2: Diagnose — wait for collector to observe stress processes dominating
    eprintln!("[agent] waiting 6s for collector to sample stress processes...");
    thread::sleep(Duration::from_secs(6));
    eprintln!("[agent] calling process_blame...");
    let blame = mcp.call_tool("process_blame");
    let (culprit_name, blame_correct) = if let Some(ref blame_data) = blame {
        let data = blame_data.get("data").unwrap_or(blame_data);
        let name = data
            .get("culprit_group")
            .and_then(|g| g.get("name"))
            .or_else(|| data.get("culprit").and_then(|c| c.get("cmd")))
            .and_then(|n| n.as_str())
            .unwrap_or("unknown")
            .to_string();
        let fix = data
            .get("fix")
            .and_then(|f| f.as_str())
            .unwrap_or("");
        let correct = name == "yes"
            || name == "dd"
            || name.contains("yes")
            || name.contains("dd");
        eprintln!("[agent] culprit: {} (correct: {})", name, correct);
        eprintln!("[agent] fix: {:?}", fix);
        (name, correct)
    } else {
        eprintln!("[agent] process_blame returned no data");
        (String::from("unknown"), false)
    };

    // Step 3: Fix
    eprintln!("[agent] applying fix: killing stress...");
    kill_all(&mut stress);

    // Step 4: Verify via hw_snapshot
    thread::sleep(Duration::from_secs(5));
    let post_snap = mcp.call_tool("hw_snapshot");
    let post_cpu = post_snap
        .as_ref()
        .and_then(|s| s.get("data"))
        .and_then(|d| d.get("cpu_usage_pct"))
        .and_then(|c| c.as_f64())
        .unwrap_or(-1.0);
    eprintln!("[agent] post-fix CPU: {:.1}%", post_cpu);

    let recovered = run_proxy_task();
    let recovery_factor = recovered / baseline;

    let _ = axon.kill();
    let _ = axon.wait();

    eprintln!("\n[agent] === PROACTIVE AGENT RESULTS ===");
    eprintln!("[agent] Baseline CPU:  {:.1}%", baseline_cpu);
    eprintln!("[agent] Stressed CPU:  {:.1}%", detected_cpu);
    eprintln!("[agent] Post-fix CPU:  {:.1}%", post_cpu);
    if let Some(d) = detect_time {
        eprintln!("[agent] Detect time:   {:.1}s (via polling)", d.as_secs_f64());
    }
    eprintln!("[agent] Culprit:       {}", culprit_name);
    eprintln!("[agent] Blame correct: {}", blame_correct);
    eprintln!("[agent] Recovery:      {:.2}s ({:.1}x)", recovered, recovery_factor);

    assert!(detect_time.is_some(), "Should detect degradation via polling");
    assert!(
        recovery_factor < 2.0,
        "Recovery too slow: {:.1}x",
        recovery_factor
    );
    assert!(blame_correct, "Blame should identify stress, got: {}", culprit_name);
}

/// Monitoring agent: collects alerts and snapshots over a stress window, then produces
/// a summary report. This simulates an observability agent that logs everything for review.
#[test]
#[ignore = "live hardware; ~50s; run with: cargo test -p axon --test agent_loop -- --ignored --nocapture"]
fn agent_monitor() {
    let bin = env!("CARGO_BIN_EXE_axon");
    eprintln!("\n=== MONITORING AGENT (collect + report) ===\n");

    let config_dir = TempDir::new().expect("tmpdir");
    let data_dir = TempDir::new().expect("tmpdir");
    let (webhook_url, rx_webhook) = start_webhook_receiver();
    write_dispatch_config(config_dir.path(), &webhook_url);

    let (mut mcp, mut axon) = McpClient::connect(bin, config_dir.path(), data_dir.path());
    eprintln!("[agent] MCP connected");

    // Warmup
    eprintln!("[agent] warming up (12s)...");
    thread::sleep(Duration::from_secs(12));
    mcp.drain_notifications();
    drain_webhook_alerts(&rx_webhook);

    // Collect: start stress, gather data for 20 seconds
    eprintln!("[agent] starting stress + monitoring for 20s...");
    let mut stress = spawn_cpu_stress();
    let monitor_start = Instant::now();
    let monitor_duration = Duration::from_secs(20);

    let mut snapshots: Vec<serde_json::Value> = Vec::new();
    let mut webhooks: Vec<serde_json::Value> = Vec::new();
    let mut notifications: Vec<serde_json::Value> = Vec::new();

    while monitor_start.elapsed() < monitor_duration {
        // Collect snapshot every 4 seconds
        if let Some(snap) = mcp.call_tool("hw_snapshot") {
            snapshots.push(snap);
        }

        // Collect any webhooks
        while let Ok(body) = rx_webhook.try_recv() {
            if let Ok(v) = serde_json::from_str(&body) {
                webhooks.push(v);
            }
        }

        // Collect any MCP notifications
        while let Some(notif) = mcp.wait_notification(Duration::from_millis(100)) {
            notifications.push(notif);
        }

        thread::sleep(Duration::from_secs(2));
    }

    // One final process_blame for the report
    let blame = mcp.call_tool("process_blame");

    // Stop stress
    kill_all(&mut stress);
    thread::sleep(Duration::from_secs(3));

    // Post-stress snapshot
    let post_snap = mcp.call_tool("hw_snapshot");
    if let Some(snap) = post_snap {
        snapshots.push(snap);
    }

    let _ = axon.kill();
    let _ = axon.wait();

    // Build report
    let cpu_readings: Vec<f64> = snapshots
        .iter()
        .filter_map(|s| {
            s.get("data")
                .and_then(|d| d.get("cpu_usage_pct"))
                .and_then(|c| c.as_f64())
        })
        .collect();
    let cpu_avg = if cpu_readings.is_empty() {
        0.0
    } else {
        cpu_readings.iter().sum::<f64>() / cpu_readings.len() as f64
    };
    let cpu_max = cpu_readings
        .iter()
        .cloned()
        .fold(f64::NEG_INFINITY, f64::max);

    let alert_types: Vec<String> = webhooks
        .iter()
        .filter_map(|w| {
            w.get("alert_type")
                .and_then(|a| a.as_str())
                .map(String::from)
        })
        .collect();

    let culprit = blame
        .as_ref()
        .and_then(|b| b.get("data"))
        .and_then(|d| d.get("culprit_group"))
        .and_then(|g| g.get("name"))
        .and_then(|n| n.as_str())
        .unwrap_or("unknown");

    let impact = blame
        .as_ref()
        .and_then(|b| b.get("data"))
        .and_then(|d| d.get("impact_level"))
        .and_then(|l| l.as_str())
        .unwrap_or("unknown");

    eprintln!("\n[agent] === MONITORING REPORT ===");
    eprintln!("[agent] Duration:       20s observation window");
    eprintln!("[agent] Snapshots:      {}", snapshots.len());
    eprintln!("[agent] Webhooks:       {}", webhooks.len());
    eprintln!("[agent] MCP notifs:     {}", notifications.len());
    eprintln!("[agent] CPU avg:        {:.1}%", cpu_avg);
    eprintln!("[agent] CPU max:        {:.1}%", cpu_max);
    eprintln!("[agent] Alert types:    {:?}", alert_types);
    eprintln!("[agent] Top culprit:    {}", culprit);
    eprintln!("[agent] Impact level:   {}", impact);

    // Assertions: monitoring agent should have collected meaningful data
    assert!(
        !snapshots.is_empty(),
        "Should have collected at least one snapshot"
    );
    assert!(
        cpu_max > 50.0,
        "CPU max should be elevated under stress, got {:.1}%",
        cpu_max
    );
    assert!(
        !webhooks.is_empty() || !notifications.is_empty(),
        "Should have received at least one alert (webhook or notification)"
    );
    assert!(
        blame.is_some(),
        "process_blame should return data during stress"
    );
}
