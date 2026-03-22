use std::sync::Arc;

use axon_core::alert_config::{
    apply_cli_overrides, AlertDispatchConfig, AlertFilters, ChannelConfig,
};
use axon_core::alert_dispatch::{AlertDispatcher, WebhookPayload};
use axon_core::alerts::{detect_alerts, AlertContext};
use axon_core::persistence;
use axon_core::types::*;
use chrono::Utc;
use tempfile::NamedTempFile;
use tokio::sync::mpsc;

fn test_db() -> persistence::DbHandle {
    let tmp = NamedTempFile::new().unwrap();
    persistence::open(tmp.path().to_path_buf()).unwrap()
}

// ── Alert Detection Integration ─────────────────────────────────────────────

#[test]
fn test_alert_detection_produces_rich_metadata() {
    let culprit = ProcessInfo {
        pid: 5678,
        cmd: "node".to_string(),
        cpu_pct: 90.0,
        ram_gb: 3.5,
        blame_score: 0.95,
    };
    let group = ProcessGroup {
        name: "Node.js".to_string(),
        process_count: 5,
        total_cpu_pct: 150.0,
        total_ram_gb: 4.0,
        blame_score: 0.9,
        top_pid: 5678,
        pids: vec![5678, 5679, 5680],
    };

    let ctx = AlertContext {
        prev_ram_pressure: &RamPressure::Normal,
        ram_pressure: &RamPressure::Critical,
        prev_throttling: false,
        throttling: true,
        die_temp: Some(98.0),
        ram_used_gb: 7.8,
        ram_total_gb: 8.0,
        cpu_pct: 92.0,
        prev_cpu_saturated: false,
        cpu_saturated: true,
        prev_disk_pressure: &DiskPressure::Normal,
        disk_pressure: &DiskPressure::Normal,
        disk_used_gb: 250.0,
        disk_total_gb: 500.0,
        prev_impact_level: &ImpactLevel::Degrading,
        impact_level: &ImpactLevel::Critical,
        impact_message: "System is critically overloaded.",
        culprit: Some(&culprit),
        culprit_group: Some(&group),
    };

    let alerts = detect_alerts(&ctx);
    assert_eq!(
        alerts.len(),
        4,
        "should produce 4 alerts: RAM + throttle + CPU saturation + impact"
    );

    // Check RAM pressure alert
    let ram_alert = alerts
        .iter()
        .find(|a| a.alert_type == AlertType::MemoryPressure)
        .unwrap();
    assert_eq!(ram_alert.severity, AlertSeverity::Critical);
    assert!(ram_alert.metadata.ram_pct.unwrap() > 95.0);
    assert_eq!(ram_alert.metadata.cpu_pct, Some(92.0));
    assert!(ram_alert.metadata.culprit.is_some());
    assert_eq!(ram_alert.metadata.culprit.as_ref().unwrap().cmd, "node");
    assert!(ram_alert.metadata.culprit_group.is_some());
    assert_eq!(
        ram_alert.metadata.culprit_group.as_ref().unwrap().name,
        "Node.js"
    );

    // Check thermal alert
    let thermal = alerts
        .iter()
        .find(|a| a.alert_type == AlertType::ThermalThrottle)
        .unwrap();
    assert_eq!(thermal.severity, AlertSeverity::Critical);
    assert_eq!(thermal.metadata.temp_c, Some(98.0));

    // Check impact escalation
    let impact = alerts
        .iter()
        .find(|a| a.alert_type == AlertType::ImpactEscalation)
        .unwrap();
    assert_eq!(impact.severity, AlertSeverity::Critical);
    assert_eq!(impact.message, "System is critically overloaded.");
}

// ── Alert Persistence Roundtrip ─────────────────────────────────────────────

#[test]
fn test_alert_persistence_roundtrip() {
    let db = test_db();

    // Insert 3 different alerts
    let alerts = vec![
        Alert {
            severity: AlertSeverity::Warning,
            alert_type: AlertType::MemoryPressure,
            message: "RAM pressure elevated to warn.".to_string(),
            ts: chrono::Utc::now(),
            metadata: AlertMetadata {
                ram_pct: Some(75.0),
                cpu_pct: Some(45.0),
                temp_c: Some(65.0),
                disk_pct: None,
                culprit: Some(ProcessInfo {
                    pid: 100,
                    cmd: "chrome".to_string(),
                    cpu_pct: 30.0,
                    ram_gb: 2.0,
                    blame_score: 0.5,
                }),
                culprit_group: None,
            },
        },
        Alert {
            severity: AlertSeverity::Critical,
            alert_type: AlertType::ThermalThrottle,
            message: "CPU thermal throttling active (99C).".to_string(),
            ts: chrono::Utc::now(),
            metadata: AlertMetadata {
                ram_pct: Some(60.0),
                cpu_pct: Some(95.0),
                temp_c: Some(99.0),
                disk_pct: None,
                culprit: None,
                culprit_group: None,
            },
        },
        Alert {
            severity: AlertSeverity::Critical,
            alert_type: AlertType::ImpactEscalation,
            message: "System is at its limit.".to_string(),
            ts: chrono::Utc::now(),
            metadata: AlertMetadata {
                ram_pct: Some(90.0),
                cpu_pct: Some(88.0),
                temp_c: None,
                disk_pct: None,
                culprit: None,
                culprit_group: None,
            },
        },
    ];

    for alert in &alerts {
        persistence::insert_alert(&db, alert);
    }

    // Query all back
    let result = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(result.len(), 3, "should have 3 alerts");

    // Query by severity
    let critical = persistence::query_alerts(&db, 3600, Some("critical"), None, 100).unwrap();
    assert_eq!(critical.len(), 2, "should have 2 critical alerts");

    let warning = persistence::query_alerts(&db, 3600, Some("warning"), None, 100).unwrap();
    assert_eq!(warning.len(), 1, "should have 1 warning alert");

    // Query by type
    let thermal =
        persistence::query_alerts(&db, 3600, None, Some("thermal_throttle"), 100).unwrap();
    assert_eq!(thermal.len(), 1, "should have 1 thermal throttle alert");
    assert_eq!(thermal[0].message, "CPU thermal throttling active (99C).");
    assert_eq!(thermal[0].metadata.temp_c, Some(99.0));

    // Query with both filters
    let critical_memory =
        persistence::query_alerts(&db, 3600, Some("critical"), Some("impact_escalation"), 100)
            .unwrap();
    assert_eq!(critical_memory.len(), 1);
    assert_eq!(critical_memory[0].message, "System is at its limit.");

    // Query with limit
    let limited = persistence::query_alerts(&db, 3600, None, None, 2).unwrap();
    assert_eq!(limited.len(), 2, "should return only 2 with limit");
}

// ── Alert Config Integration ────────────────────────────────────────────────

#[test]
fn test_config_file_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("alert-dispatch.json");

    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Mcp {
                id: "mcp_client".to_string(),
                filters: AlertFilters {
                    severity: vec![],
                    alert_types: vec!["*".to_string()],
                },
            },
            ChannelConfig::Webhook {
                id: "openclaw".to_string(),
                url: "http://localhost:3000/alerts".to_string(),
                filters: AlertFilters {
                    severity: vec!["critical".to_string()],
                    alert_types: vec![
                        "memory_pressure".to_string(),
                        "thermal_throttle".to_string(),
                    ],
                },
            },
        ],
    };

    // Write config file
    let json = serde_json::to_string_pretty(&config).unwrap();
    std::fs::write(&config_path, &json).unwrap();

    // Load it back
    let loaded = axon_core::alert_config::load_config(Some(&dir.path().to_path_buf()));
    assert_eq!(loaded.channels.len(), 2);
    assert_eq!(loaded.channels[0].id(), "mcp_client");
    assert_eq!(loaded.channels[1].id(), "openclaw");

    // Verify filters loaded correctly
    let openclaw_filters = loaded.channels[1].filters();
    assert_eq!(openclaw_filters.severity, vec!["critical"]);
    assert_eq!(
        openclaw_filters.alert_types,
        vec!["memory_pressure", "thermal_throttle"]
    );

    // Test filter behavior
    assert!(openclaw_filters.accepts(&AlertSeverity::Critical, &AlertType::MemoryPressure));
    assert!(!openclaw_filters.accepts(&AlertSeverity::Warning, &AlertType::MemoryPressure));
    assert!(!openclaw_filters.accepts(&AlertSeverity::Critical, &AlertType::ImpactEscalation));
}

// ── Dispatcher Integration (MCP channel only, no real HTTP) ─────────────────

#[tokio::test]
async fn test_dispatcher_mcp_only() {
    let config = AlertDispatchConfig::default();
    let dispatcher = AlertDispatcher::new(config);
    let db = test_db();

    let alert = Alert {
        severity: AlertSeverity::Warning,
        alert_type: AlertType::MemoryPressure,
        message: "test alert".to_string(),
        ts: chrono::Utc::now(),
        metadata: AlertMetadata {
            ram_pct: Some(75.0),
            cpu_pct: Some(50.0),
            temp_c: None,
            disk_pct: None,
            culprit: None,
            culprit_group: None,
        },
    };

    let send_via_mcp = dispatcher.dispatch(&alert, &db).await;
    assert!(
        send_via_mcp,
        "MCP channel should accept all alerts by default"
    );

    // Verify alert was persisted
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(stored.len(), 1);
    assert_eq!(stored[0].message, "test alert");
}

#[tokio::test]
async fn test_dispatcher_with_filtered_webhook() {
    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Mcp {
            id: "mcp".to_string(),
            filters: AlertFilters {
                severity: vec!["critical".to_string()],
                alert_types: vec![],
            },
        }],
    };
    let dispatcher = AlertDispatcher::new(config);
    let db = test_db();

    // Warning alert should NOT be sent via MCP (filtered to critical only)
    let warning = Alert {
        severity: AlertSeverity::Warning,
        alert_type: AlertType::MemoryPressure,
        message: "warning alert".to_string(),
        ts: chrono::Utc::now(),
        metadata: AlertMetadata {
            ram_pct: None,
            cpu_pct: None,
            temp_c: None,
            disk_pct: None,
            culprit: None,
            culprit_group: None,
        },
    };
    let send = dispatcher.dispatch(&warning, &db).await;
    assert!(
        !send,
        "warning should be filtered out for critical-only MCP channel"
    );

    // Critical alert SHOULD be sent via MCP
    let critical = Alert {
        severity: AlertSeverity::Critical,
        alert_type: AlertType::ThermalThrottle,
        message: "critical alert".to_string(),
        ts: chrono::Utc::now(),
        metadata: AlertMetadata {
            ram_pct: None,
            cpu_pct: None,
            temp_c: Some(99.0),
            disk_pct: None,
            culprit: None,
            culprit_group: None,
        },
    };
    let send = dispatcher.dispatch(&critical, &db).await;
    assert!(send, "critical should pass filter");

    // Both should still be persisted regardless of filter
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(
        stored.len(),
        2,
        "both alerts should be persisted even if filtered"
    );
}

// ── Real HTTP Webhook Tests ─────────────────────────────────────────────────

/// Parsed webhook HTTP request (what `reqwest` actually sends).
#[derive(Debug, Clone)]
struct CapturedWebhookHttp {
    request_line: String,
    method: String,
    path: String,
    content_type: Option<String>,
    body: String,
}

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn parse_content_length_and_type(headers_block: &str) -> (Option<usize>, Option<String>) {
    let mut content_length = None;
    let mut content_type = None;
    for line in headers_block.split("\r\n").skip(1) {
        let lower = line.to_ascii_lowercase();
        if lower.starts_with("content-length:") {
            content_length = line.split(':').nth(1).and_then(|s| s.trim().parse().ok());
        } else if lower.starts_with("content-type:") {
            content_type = line.split(':').nth(1).map(|s| s.trim().to_string());
        }
    }
    (content_length, content_type)
}

/// Read a full HTTP/1.x request: headers plus body per Content-Length (handles split TCP reads).
async fn read_full_http_request(
    socket: &mut tokio::net::TcpStream,
) -> std::io::Result<CapturedWebhookHttp> {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let mut buf: Vec<u8> = Vec::new();
    let mut tmp = [0u8; 4096];

    loop {
        if let Some(end_headers) = find_headers_end(&buf) {
            let header_str = std::str::from_utf8(&buf[..end_headers])
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            let first_line = header_str.split("\r\n").next().unwrap_or("").to_string();
            let parts: Vec<&str> = first_line.split_whitespace().collect();
            let method = parts.first().unwrap_or(&"").to_string();
            let path = parts.get(1).unwrap_or(&"").to_string();

            let headers_only = header_str;
            let (cl_opt, ctype) = parse_content_length_and_type(headers_only);
            let body_start = end_headers + 4;
            let cl = cl_opt.unwrap_or(0);
            let need_total = body_start + cl;

            while buf.len() < need_total {
                let n = socket.read(&mut tmp).await?;
                if n == 0 {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::UnexpectedEof,
                        "incomplete body",
                    ));
                }
                buf.extend_from_slice(&tmp[..n]);
            }

            let body_bytes = &buf[body_start..need_total];
            let body = String::from_utf8_lossy(body_bytes).to_string();

            let response = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
            let _ = socket.write_all(response.as_bytes()).await;

            return Ok(CapturedWebhookHttp {
                request_line: first_line,
                method,
                path,
                content_type: ctype,
                body,
            });
        }

        let n = socket.read(&mut tmp).await?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "closed before headers",
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

/// Start a TCP listener; each connection yields one fully captured HTTP request (body parsed).
async fn start_webhook_receiver() -> (String, mpsc::Receiver<CapturedWebhookHttp>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{}/alerts", port);

    let (tx, rx) = mpsc::channel::<CapturedWebhookHttp>(32);

    tokio::spawn(async move {
        loop {
            let Ok((mut socket, _)) = listener.accept().await else {
                break;
            };
            let tx = tx.clone();
            tokio::spawn(async move {
                if let Ok(captured) = read_full_http_request(&mut socket).await {
                    let _ = tx.send(captured).await;
                }
            });
        }
    });

    (url, rx)
}

fn make_test_alert(severity: AlertSeverity, alert_type: AlertType, msg: &str) -> Alert {
    Alert {
        severity,
        alert_type,
        message: msg.to_string(),
        ts: chrono::Utc::now(),
        metadata: AlertMetadata {
            ram_pct: Some(85.0),
            cpu_pct: Some(72.0),
            temp_c: Some(65.0),
            disk_pct: None,
            culprit: Some(ProcessInfo {
                pid: 1234,
                cmd: "chrome".to_string(),
                cpu_pct: 50.0,
                ram_gb: 2.5,
                blame_score: 0.8,
            }),
            culprit_group: None,
        },
    }
}

#[tokio::test]
async fn test_webhook_delivery_real_http() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Mcp {
                id: "mcp".to_string(),
                filters: AlertFilters::default(),
            },
            ChannelConfig::Webhook {
                id: "test_receiver".to_string(),
                url: url.clone(),
                filters: AlertFilters::default(),
            },
        ],
    };
    let dispatcher = AlertDispatcher::new(config);

    let alert = make_test_alert(
        AlertSeverity::Critical,
        AlertType::MemoryPressure,
        "RAM pressure critical (7.5/8.0GB). System may freeze.",
    );

    // Dispatch the alert
    let send_mcp = dispatcher.dispatch(&alert, &db).await;
    assert!(send_mcp, "MCP channel should also be triggered");

    // Wait for the webhook POST to arrive (full HTTP parse — works with split packets)
    let captured = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("webhook should arrive within 5s")
        .expect("should receive captured request");

    assert_eq!(
        captured.method, "POST",
        "reqwest should POST: {}",
        captured.request_line
    );
    assert!(
        captured.path.contains("/alerts"),
        "path should include /alerts: {}",
        captured.path
    );
    let ctype = captured.content_type.as_deref().unwrap_or("");
    assert!(
        ctype.contains("application/json"),
        "Content-Type should be JSON, got: {:?}",
        captured.content_type
    );

    // Parse and verify the payload
    let payload: WebhookPayload = serde_json::from_str(&captured.body).unwrap_or_else(|e| {
        panic!(
            "failed to parse webhook body as JSON: {}\nbody: {}",
            e, captured.body
        )
    });

    assert_eq!(payload.alert_type, "memory_pressure");
    assert_eq!(payload.severity, "critical");
    assert!(payload.message.contains("RAM pressure critical"));
    assert_eq!(payload.metrics.ram_pct, Some(85.0));
    assert_eq!(payload.metrics.cpu_pct, Some(72.0));
    assert_eq!(payload.metrics.temp_c, Some(65.0));
    assert!(payload.culprit.is_some());
    assert_eq!(payload.culprit.as_ref().unwrap().name, "chrome");
    assert_eq!(payload.culprit.as_ref().unwrap().pid, 1234);

    // Verify alert was also persisted to DB
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(stored.len(), 1);
}

#[tokio::test]
async fn test_webhook_fire_and_forget_no_listener() {
    // Point to a port where nothing is listening
    let db = test_db();
    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "dead_endpoint".to_string(),
            url: "http://127.0.0.1:1/alerts".to_string(), // port 1 = nothing listening
            filters: AlertFilters::default(),
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    let alert = make_test_alert(
        AlertSeverity::Warning,
        AlertType::ThermalThrottle,
        "test fire-and-forget",
    );

    // This should NOT hang or crash — fire-and-forget
    let result = tokio::time::timeout(
        std::time::Duration::from_secs(3),
        dispatcher.dispatch(&alert, &db),
    )
    .await;

    assert!(
        result.is_ok(),
        "dispatch should complete within 3s even with dead endpoint"
    );

    // Alert should still be persisted to DB even though webhook failed
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(stored.len(), 1, "alert persisted despite webhook failure");
}

#[tokio::test]
async fn test_webhook_multiple_endpoints() {
    let (url1, mut rx1) = start_webhook_receiver().await;
    let (url2, mut rx2) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Webhook {
                id: "endpoint_a".to_string(),
                url: url1,
                filters: AlertFilters::default(),
            },
            ChannelConfig::Webhook {
                id: "endpoint_b".to_string(),
                url: url2,
                filters: AlertFilters::default(),
            },
        ],
    };
    let dispatcher = AlertDispatcher::new(config);

    let alert = make_test_alert(
        AlertSeverity::Critical,
        AlertType::ThermalThrottle,
        "thermal throttle alert for multi-endpoint test",
    );

    dispatcher.dispatch(&alert, &db).await;

    // Both endpoints should receive the alert
    let cap1 = tokio::time::timeout(std::time::Duration::from_secs(5), rx1.recv())
        .await
        .expect("endpoint_a should receive within 5s")
        .expect("should have body");
    let cap2 = tokio::time::timeout(std::time::Duration::from_secs(5), rx2.recv())
        .await
        .expect("endpoint_b should receive within 5s")
        .expect("should have body");

    assert_eq!(cap1.method, "POST");
    assert_eq!(cap2.method, "POST");

    let p1: WebhookPayload = serde_json::from_str(&cap1.body).unwrap();
    let p2: WebhookPayload = serde_json::from_str(&cap2.body).unwrap();

    assert_eq!(p1.alert_type, "thermal_throttle");
    assert_eq!(p2.alert_type, "thermal_throttle");
    assert_eq!(p1.severity, "critical");
    assert_eq!(p2.severity, "critical");
}

#[tokio::test]
async fn test_webhook_filter_blocks_severity() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "critical_only".to_string(),
            url,
            filters: AlertFilters {
                severity: vec!["critical".to_string()],
                alert_types: vec![],
            },
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    // Send a warning alert — should be filtered out
    let warning = make_test_alert(
        AlertSeverity::Warning,
        AlertType::MemoryPressure,
        "this should be filtered",
    );
    dispatcher.dispatch(&warning, &db).await;

    // Try to receive — should timeout (no POST sent)
    let result = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(
        result.is_err(),
        "warning should NOT arrive at critical-only webhook"
    );

    // Now send a critical alert — should pass filter
    let critical = make_test_alert(
        AlertSeverity::Critical,
        AlertType::MemoryPressure,
        "this should pass",
    );
    dispatcher.dispatch(&critical, &db).await;

    let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("critical should arrive")
        .expect("should have body");
    assert_eq!(cap.method, "POST");
    let payload: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(payload.severity, "critical");
    assert_eq!(payload.message, "this should pass");
}

#[tokio::test]
async fn test_webhook_filter_blocks_alert_type() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "thermal_only".to_string(),
            url,
            filters: AlertFilters {
                severity: vec![],
                alert_types: vec!["thermal_throttle".to_string()],
            },
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    // Memory pressure alert — should be filtered out
    let mem_alert = make_test_alert(
        AlertSeverity::Critical,
        AlertType::MemoryPressure,
        "memory alert filtered",
    );
    dispatcher.dispatch(&mem_alert, &db).await;

    let result = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(
        result.is_err(),
        "memory_pressure should NOT arrive at thermal_only webhook"
    );

    // Thermal alert — should pass
    let thermal = make_test_alert(
        AlertSeverity::Critical,
        AlertType::ThermalThrottle,
        "thermal passes",
    );
    dispatcher.dispatch(&thermal, &db).await;

    let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("thermal should arrive")
        .expect("body");
    assert_eq!(cap.method, "POST");
    let payload: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(payload.alert_type, "thermal_throttle");
}

#[tokio::test]
async fn test_webhook_concurrent_alerts_all_delivered() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "burst_test".to_string(),
            url,
            filters: AlertFilters::default(),
        }],
    };
    let dispatcher = Arc::new(AlertDispatcher::new(config));

    // Fire 10 alerts rapidly
    for i in 0..10 {
        let alert = make_test_alert(
            AlertSeverity::Critical,
            AlertType::MemoryPressure,
            &format!("burst alert {}", i),
        );
        dispatcher.dispatch(&alert, &db).await;
    }

    // Collect all received payloads
    let mut received = Vec::new();
    for _ in 0..10 {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(cap)) => received.push(cap),
            _ => break,
        }
    }

    assert_eq!(
        received.len(),
        10,
        "all 10 alerts should be delivered; got {}",
        received.len()
    );

    // Verify each is valid JSON
    for cap in &received {
        assert_eq!(cap.method, "POST");
        let payload: WebhookPayload = serde_json::from_str(&cap.body)
            .unwrap_or_else(|e| panic!("invalid JSON in burst: {}\nbody: {}", e, cap.body));
        assert_eq!(payload.alert_type, "memory_pressure");
    }

    // All 10 should also be persisted
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(stored.len(), 10);
}

// ── Full Pipeline Test: detect → dispatch → webhook → DB ────────────────────

#[tokio::test]
async fn test_full_pipeline_detect_dispatch_webhook_persist() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Mcp {
                id: "mcp".to_string(),
                filters: AlertFilters::default(),
            },
            ChannelConfig::Webhook {
                id: "openclaw".to_string(),
                url,
                filters: AlertFilters {
                    severity: vec!["critical".to_string()],
                    alert_types: vec![
                        "memory_pressure".to_string(),
                        "thermal_throttle".to_string(),
                    ],
                },
            },
        ],
    };
    let dispatcher = AlertDispatcher::new(config);

    // Simulate a real scenario: RAM goes critical + thermal throttle
    let culprit = ProcessInfo {
        pid: 9999,
        cmd: "cargo".to_string(),
        cpu_pct: 400.0,
        ram_gb: 5.0,
        blame_score: 0.95,
    };

    let ctx = AlertContext {
        prev_ram_pressure: &RamPressure::Normal,
        ram_pressure: &RamPressure::Critical,
        prev_throttling: false,
        throttling: true,
        die_temp: Some(99.0),
        ram_used_gb: 7.5,
        ram_total_gb: 8.0,
        cpu_pct: 95.0,
        prev_cpu_saturated: false,
        cpu_saturated: true,
        prev_disk_pressure: &DiskPressure::Normal,
        disk_pressure: &DiskPressure::Normal,
        disk_used_gb: 250.0,
        disk_total_gb: 500.0,
        prev_impact_level: &ImpactLevel::Degrading,
        impact_level: &ImpactLevel::Critical,
        impact_message: "System is critically overloaded.",
        culprit: Some(&culprit),
        culprit_group: None,
    };

    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 4, "should detect 4 alerts (RAM + throttle + CPU saturation + impact)");

    // Dispatch all alerts
    let mut mcp_count = 0;
    for alert in &alerts {
        if dispatcher.dispatch(alert, &db).await {
            mcp_count += 1;
        }
    }
    assert_eq!(
        mcp_count, 4,
        "MCP channel has default wildcard, all 4 should pass"
    );

    // The webhook filter is: critical + (memory_pressure OR thermal_throttle)
    // Alert 1: Critical MemoryPressure → PASS
    // Alert 2: Critical ThermalThrottle → PASS
    // Alert 3: Warning CpuSaturation → BLOCKED (severity not critical)
    // Alert 4: Critical ImpactEscalation → BLOCKED (type not in filter)
    // So webhook should receive exactly 2

    let mut webhook_payloads = Vec::new();
    for _ in 0..2 {
        match tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv()).await {
            Ok(Some(cap)) => {
                assert_eq!(cap.method, "POST");
                let p: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
                webhook_payloads.push(p);
            }
            _ => break,
        }
    }
    assert_eq!(
        webhook_payloads.len(),
        2,
        "webhook should receive exactly 2 alerts"
    );

    // Verify the 3rd (ImpactEscalation) was NOT sent
    let third_attempt =
        tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(
        third_attempt.is_err(),
        "impact_escalation should NOT be sent to webhook"
    );

    let types: Vec<&str> = webhook_payloads
        .iter()
        .map(|p| p.alert_type.as_str())
        .collect();
    assert!(types.contains(&"memory_pressure"));
    assert!(types.contains(&"thermal_throttle"));

    // Verify culprit present in payloads
    for p in &webhook_payloads {
        assert!(p.culprit.is_some());
        assert_eq!(p.culprit.as_ref().unwrap().name, "cargo");
        assert_eq!(p.culprit.as_ref().unwrap().pid, 9999);
    }

    // All 4 alerts should be persisted to DB (persistence is unfiltered)
    let stored = persistence::query_alerts(&db, 3600, None, None, 100).unwrap();
    assert_eq!(stored.len(), 4, "all 4 alerts should be in the database");
}

// ── Stronger integration: HTTP shape, MCP flag, CLI merge ─────────────────────

#[tokio::test]
async fn test_webhook_only_config_returns_false_for_mcp() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "solo".to_string(),
            url,
            filters: AlertFilters::default(),
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    let alert = make_test_alert(
        AlertSeverity::Warning,
        AlertType::MemoryPressure,
        "webhook-only path",
    );
    let send_via_mcp = dispatcher.dispatch(&alert, &db).await;
    assert!(
        !send_via_mcp,
        "with no MCP channel, dispatcher should not signal MCP notifications"
    );

    let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("webhook POST should arrive")
        .expect("captured request");
    assert_eq!(cap.method, "POST");
    let p: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(p.message, "webhook-only path");
}

#[tokio::test]
async fn test_cli_override_adds_webhook_and_posts_json() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let merged = apply_cli_overrides(
        AlertDispatchConfig::default(),
        &[("openclaw".to_string(), url)],
        &[],
    );
    assert!(
        merged.channels.iter().any(|c| c.id() == "openclaw"),
        "CLI should inject webhook channel"
    );
    assert!(
        merged
            .channels
            .iter()
            .any(|c| matches!(c, ChannelConfig::Mcp { .. })),
        "default MCP channel should remain"
    );

    let dispatcher = AlertDispatcher::new(merged);
    let alert = make_test_alert(
        AlertSeverity::Critical,
        AlertType::ThermalThrottle,
        "cli merge smoke",
    );
    assert!(
        dispatcher.dispatch(&alert, &db).await,
        "default MCP still accepts critical thermal"
    );

    let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("webhook from CLI-injected URL")
        .expect("captured");
    assert_eq!(cap.method, "POST");
    assert!(cap
        .content_type
        .as_deref()
        .unwrap_or("")
        .contains("application/json"));
    let p: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
    assert_eq!(p.alert_type, "thermal_throttle");
    assert_eq!(p.message, "cli merge smoke");
}

#[tokio::test]
async fn test_webhook_json_body_has_null_culprit_when_absent() {
    let (url, mut rx) = start_webhook_receiver().await;
    let db = test_db();

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "w".to_string(),
            url,
            filters: AlertFilters::default(),
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    let mut alert = make_test_alert(
        AlertSeverity::Warning,
        AlertType::ImpactEscalation,
        "no culprit case",
    );
    alert.metadata.culprit = None;

    dispatcher.dispatch(&alert, &db).await;

    let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("POST")
        .expect("cap");
    let v: serde_json::Value = serde_json::from_str(&cap.body).unwrap();
    assert!(
        v.get("culprit").map_or(true, |c| c.is_null()),
        "culprit should be null or absent: {}",
        cap.body
    );
}

// ── Phase 1 integration: persistence + dispatcher matrix ────────────────────

#[test]
fn test_alert_persistence_pruning() {
    let tmp = NamedTempFile::new().unwrap();
    let path = tmp.path().to_path_buf();
    {
        let db = persistence::open(path.clone()).unwrap();
        let old_ts = Utc::now() - chrono::Duration::days(40);
        let alert = Alert {
            severity: AlertSeverity::Warning,
            alert_type: AlertType::MemoryPressure,
            message: "old".to_string(),
            ts: old_ts,
            metadata: AlertMetadata {
                ram_pct: None,
                cpu_pct: None,
                temp_c: None,
                disk_pct: None,
                culprit: None,
                culprit_group: None,
            },
        };
        persistence::insert_alert(&db, &alert);
        assert_eq!(persistence::count_alerts(&db).unwrap(), 1);
    }
    let db2 = persistence::open(path).unwrap();
    assert_eq!(persistence::count_alerts(&db2).unwrap(), 0);
}

#[test]
fn test_alert_persistence_concurrent() {
    use std::thread;
    let db = test_db();
    let mut hs = vec![];
    for _ in 0..2 {
        let d = db.clone();
        hs.push(thread::spawn(move || {
            for i in 0..25 {
                let alert = make_test_alert(
                    AlertSeverity::Warning,
                    AlertType::MemoryPressure,
                    &format!("t{i}"),
                );
                persistence::insert_alert(&d, &alert);
            }
        }));
    }
    for h in hs {
        h.join().unwrap();
    }
    assert_eq!(persistence::count_alerts(&db).unwrap(), 50);
}

#[tokio::test]
async fn test_dispatcher_routes_to_multiple_channels() {
    let (url1, mut rx1) = start_webhook_receiver().await;
    let (url2, mut rx2) = start_webhook_receiver().await;
    let db = test_db();
    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Mcp {
                id: "mcp".to_string(),
                filters: AlertFilters::default(),
            },
            ChannelConfig::Webhook {
                id: "w1".to_string(),
                url: url1,
                filters: AlertFilters::default(),
            },
            ChannelConfig::Webhook {
                id: "w2".to_string(),
                url: url2,
                filters: AlertFilters::default(),
            },
        ],
    };
    let d = AlertDispatcher::new(config);
    let alert = make_test_alert(
        AlertSeverity::Critical,
        AlertType::MemoryPressure,
        "broadcast",
    );
    assert!(d.dispatch(&alert, &db).await, "MCP should accept");
    for rx in [&mut rx1, &mut rx2] {
        let cap = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
            .await
            .expect("webhook")
            .expect("body");
        assert_eq!(cap.method, "POST");
        let p: WebhookPayload = serde_json::from_str(&cap.body).unwrap();
        assert_eq!(p.message, "broadcast");
    }
}

#[tokio::test]
async fn test_dispatcher_respects_per_channel_filters() {
    let (url_a, mut rx_a) = start_webhook_receiver().await;
    let (url_b, mut rx_b) = start_webhook_receiver().await;
    let db = test_db();
    let config = AlertDispatchConfig {
        channels: vec![
            ChannelConfig::Webhook {
                id: "critical_only".to_string(),
                url: url_a,
                filters: AlertFilters {
                    severity: vec!["critical".to_string()],
                    alert_types: vec![],
                },
            },
            ChannelConfig::Webhook {
                id: "all".to_string(),
                url: url_b,
                filters: AlertFilters::default(),
            },
        ],
    };
    let d = AlertDispatcher::new(config);
    let warn = make_test_alert(AlertSeverity::Warning, AlertType::MemoryPressure, "warn");
    d.dispatch(&warn, &db).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(400), rx_a.recv())
            .await
            .is_err(),
        "critical-only channel should not get warning"
    );
    let cap_b = tokio::time::timeout(std::time::Duration::from_secs(5), rx_b.recv())
        .await
        .expect("b")
        .expect("cap");
    assert_eq!(cap_b.method, "POST");
}

// ── Mock State-Machine Transition Tests ─────────────────────────────────────
// These tests simulate the collector's edge-trigger logic entirely in memory —
// no binary spawn, no sysinfo, no hardware required.

fn base_ctx<'a>(
    prev_ram: &'a RamPressure,
    ram: &'a RamPressure,
    prev_impact: &'a ImpactLevel,
    impact: &'a ImpactLevel,
) -> AlertContext<'a> {
    AlertContext {
        prev_ram_pressure: prev_ram,
        ram_pressure: ram,
        prev_throttling: false,
        throttling: false,
        die_temp: None,
        ram_used_gb: 6.0,
        ram_total_gb: 8.0,
        cpu_pct: 50.0,
        prev_cpu_saturated: false,
        cpu_saturated: false,
        prev_disk_pressure: &DiskPressure::Normal,
        disk_pressure: &DiskPressure::Normal,
        disk_used_gb: 250.0,
        disk_total_gb: 500.0,
        prev_impact_level: prev_impact,
        impact_level: impact,
        impact_message: "test",
        culprit: None,
        culprit_group: None,
    }
}

#[test]
fn test_alert_state_transitions_ram_tiers() {
    // Normal → Warn: one warning alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Warn,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].alert_type, AlertType::MemoryPressure);
    assert_eq!(alerts[0].severity, AlertSeverity::Warning);

    // Warn → Warn: no alert (stable state, no edge)
    let ctx = base_ctx(
        &RamPressure::Warn,
        &RamPressure::Warn,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert!(alerts.is_empty(), "no alert on stable Warn state");

    // Warn → Critical: one critical alert
    let ctx = base_ctx(
        &RamPressure::Warn,
        &RamPressure::Critical,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].alert_type, AlertType::MemoryPressure);
    assert_eq!(alerts[0].severity, AlertSeverity::Critical);

    // Critical → Critical: no alert
    let ctx = base_ctx(
        &RamPressure::Critical,
        &RamPressure::Critical,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert!(alerts.is_empty(), "no alert on stable Critical state");

    // Critical → Normal (recovery): resolved alert
    let ctx = base_ctx(
        &RamPressure::Critical,
        &RamPressure::Normal,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1, "recovery to Normal produces resolved alert");
    assert_eq!(alerts[0].severity, AlertSeverity::Resolved);
    assert_eq!(alerts[0].alert_type, AlertType::MemoryPressure);

    // Normal → Critical (skip Warn): one critical alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Critical,
        &ImpactLevel::Healthy,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].severity, AlertSeverity::Critical);
}

#[test]
fn test_alert_state_transitions_impact_escalation() {
    // Healthy → Degrading: no alert (Degrading does not trigger)
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Healthy,
        &ImpactLevel::Degrading,
    );
    let alerts = detect_alerts(&ctx);
    assert!(alerts.is_empty(), "Healthy→Degrading must not alert");

    // Degrading → Degrading: no alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Degrading,
        &ImpactLevel::Degrading,
    );
    let alerts = detect_alerts(&ctx);
    assert!(alerts.is_empty(), "stable Degrading must not alert");

    // Degrading → Strained: one warning alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Degrading,
        &ImpactLevel::Strained,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
    assert_eq!(alerts[0].severity, AlertSeverity::Warning);

    // Strained → Strained: no alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Strained,
        &ImpactLevel::Strained,
    );
    let alerts = detect_alerts(&ctx);
    assert!(alerts.is_empty(), "stable Strained must not alert");

    // Strained → Critical: one critical alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Strained,
        &ImpactLevel::Critical,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
    assert_eq!(alerts[0].severity, AlertSeverity::Critical);

    // Healthy → Strained (fast escalation): one warning alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Healthy,
        &ImpactLevel::Strained,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1);
    assert_eq!(alerts[0].severity, AlertSeverity::Warning);

    // Critical → Healthy (recovery): resolved alert
    let ctx = base_ctx(
        &RamPressure::Normal,
        &RamPressure::Normal,
        &ImpactLevel::Critical,
        &ImpactLevel::Healthy,
    );
    let alerts = detect_alerts(&ctx);
    assert_eq!(alerts.len(), 1, "recovery to Healthy produces resolved alert");
    assert_eq!(alerts[0].severity, AlertSeverity::Resolved);
    assert_eq!(alerts[0].alert_type, AlertType::ImpactEscalation);
}

#[test]
fn test_alert_no_regression_on_stable_state() {
    // Simulate 10 consecutive "ticks" with no state change — zero alerts throughout.
    let ram = RamPressure::Critical;
    let impact = ImpactLevel::Strained;

    for tick in 0..10 {
        let ctx = base_ctx(&ram, &ram, &impact, &impact);
        let alerts = detect_alerts(&ctx);
        assert!(
            alerts.is_empty(),
            "tick {}: stable state must produce no alerts",
            tick
        );
    }
}

#[test]
fn test_alert_full_collector_cycle_mock() {
    // Simulate a realistic collector cycle: system starts healthy, load arrives,
    // escalates, then recovers. Verify edge-trigger fires exactly once per transition.

    let mut total_alerts = 0usize;

    // Ticks 1-3: warm-up — state changes silently (prev updated, no dispatch).
    // Initialize prev directly to post-warmup state (Warn RAM, Degrading impact).
    let mut prev_ram = RamPressure::Warn;
    let mut prev_impact = ImpactLevel::Degrading;

    // Tick 4: first dispatch tick — RAM already Warn, impact Degrading (no new edges since warm-up)
    let ctx = base_ctx(
        &prev_ram,
        &RamPressure::Warn,
        &prev_impact,
        &ImpactLevel::Degrading,
    );
    let alerts = detect_alerts(&ctx);
    total_alerts += alerts.len();
    assert!(alerts.is_empty(), "tick 4: no edge after warm-up catch-up");
    prev_ram = RamPressure::Warn;
    prev_impact = ImpactLevel::Degrading;

    // Tick 5: RAM escalates to Critical (edge: Warn→Critical)
    let ctx = base_ctx(
        &prev_ram,
        &RamPressure::Critical,
        &prev_impact,
        &ImpactLevel::Degrading,
    );
    let a = detect_alerts(&ctx);
    assert_eq!(a.len(), 1, "tick 5: Warn→Critical must produce 1 alert");
    assert_eq!(a[0].alert_type, AlertType::MemoryPressure);
    total_alerts += a.len();
    prev_ram = RamPressure::Critical;

    // Tick 6: stable Critical RAM, impact escalates to Strained
    let ctx = base_ctx(
        &prev_ram,
        &RamPressure::Critical,
        &prev_impact,
        &ImpactLevel::Strained,
    );
    let a = detect_alerts(&ctx);
    assert_eq!(
        a.len(),
        1,
        "tick 6: Degrading→Strained must produce 1 alert"
    );
    assert_eq!(a[0].alert_type, AlertType::ImpactEscalation);
    total_alerts += a.len();
    prev_impact = ImpactLevel::Strained;

    // Ticks 7-8: stable (Critical RAM, Strained impact) — no alerts
    for tick in 7..=8 {
        let ctx = base_ctx(
            &prev_ram,
            &RamPressure::Critical,
            &prev_impact,
            &ImpactLevel::Strained,
        );
        let a = detect_alerts(&ctx);
        assert!(a.is_empty(), "tick {}: stable state must not alert", tick);
        total_alerts += a.len();
    }

    // Tick 9: recovery — RAM drops to Warn, impact recovers to Degrading (resolved alerts)
    let ctx = base_ctx(
        &prev_ram,
        &RamPressure::Warn,
        &prev_impact,
        &ImpactLevel::Degrading,
    );
    let a = detect_alerts(&ctx);
    // RAM: Critical→Warn = resolved, Impact: Strained→Degrading = resolved
    assert_eq!(a.len(), 2, "tick 9: recovery produces resolved alerts");
    assert!(
        a.iter().all(|alert| alert.severity == AlertSeverity::Resolved),
        "tick 9: all recovery alerts must be Resolved"
    );
    total_alerts += a.len();
    prev_ram = RamPressure::Warn;
    prev_impact = ImpactLevel::Degrading;

    // Tick 10: fully recovered — Normal RAM, Healthy impact (more resolved alerts)
    let ctx = base_ctx(
        &prev_ram,
        &RamPressure::Normal,
        &prev_impact,
        &ImpactLevel::Healthy,
    );
    let a = detect_alerts(&ctx);
    // RAM: Warn→Normal = resolved (no impact alert: Degrading→Healthy is not an escalation recovery)
    assert_eq!(a.len(), 1, "tick 10: final recovery produces resolved alert");
    assert_eq!(a[0].severity, AlertSeverity::Resolved);
    total_alerts += a.len();

    // 2 escalation alerts + 3 resolved alerts = 5 total
    assert_eq!(
        total_alerts, 5,
        "full cycle must produce 2 escalation + 3 resolved alerts"
    );
}
