//! Real HTTP webhook POST from `AlertDispatcher` (no hardware, no `axon serve`).
//! Proves JSON delivery the same way production uses it.
//!
//!   cargo test -p axon --test webhook_dispatch_smoke -- --nocapture

use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use axon_core::alert_config::{AlertDispatchConfig, AlertFilters, ChannelConfig};
use axon_core::alert_dispatch::{AlertDispatcher, WebhookPayload};
use axon_core::persistence;
use axon_core::types::{Alert, AlertMetadata, AlertSeverity, AlertType, ProcessInfo};
use chrono::Utc;
use tempfile::NamedTempFile;

fn find_headers_end(buf: &[u8]) -> Option<usize> {
    buf.windows(4).position(|w| w == b"\r\n\r\n")
}

fn read_http_post_body(stream: &mut std::net::TcpStream) -> std::io::Result<String> {
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

fn start_listener() -> (String, mpsc::Receiver<String>) {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let url = format!("http://127.0.0.1:{}/alerts", port);
    let (tx, rx) = mpsc::channel::<String>();
    thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let _ = stream.set_read_timeout(Some(Duration::from_secs(30)));
        if let Ok(body) = read_http_post_body(&mut stream) {
            let _ = tx.send(body);
        }
        let _ = stream
            .write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok");
    });
    (url, rx)
}

#[tokio::test]
async fn dispatcher_posts_webhook_json_without_hardware() {
    let (url, rx) = start_listener();

    let tmp = NamedTempFile::new().expect("tmp");
    let db = persistence::open(tmp.path().to_path_buf()).expect("db");

    let config = AlertDispatchConfig {
        channels: vec![ChannelConfig::Webhook {
            id: "smoke".to_string(),
            url: url.clone(),
            filters: AlertFilters::default(),
        }],
    };
    let dispatcher = AlertDispatcher::new(config);

    let alert = Alert {
        severity: AlertSeverity::Critical,
        alert_type: AlertType::MemoryPressure,
        message: "RAM pressure critical (7.5/8.0GB). System may freeze.".to_string(),
        ts: Utc::now(),
        metadata: AlertMetadata {
            ram_pct: Some(85.0),
            cpu_pct: Some(72.0),
            temp_c: Some(65.0),
            culprit: Some(ProcessInfo {
                pid: 1234,
                cmd: "chrome".to_string(),
                cpu_pct: 50.0,
                ram_gb: 2.5,
                blame_score: 0.8,
            }),
            culprit_group: None,
        },
    };

    let send_mcp = dispatcher.dispatch(&alert, &db).await;
    assert!(!send_mcp, "webhook-only config should not signal MCP");

    let body = tokio::task::spawn_blocking(move || rx.recv_timeout(Duration::from_secs(5)))
        .await
        .expect("join")
        .expect("webhook should arrive within 5s");

    let payload: WebhookPayload = serde_json::from_str(&body).unwrap_or_else(|e| {
        panic!("webhook JSON parse: {e}\nbody: {body}");
    });

    assert_eq!(payload.alert_type, "memory_pressure");
    assert_eq!(payload.severity, "critical");
    assert!(payload.message.contains("RAM pressure critical"));
    assert_eq!(payload.metrics.ram_pct, Some(85.0));
    assert!(payload.culprit.is_some());

    let stored = persistence::query_alerts(&db, 3600, None, None, 100).expect("query");
    assert_eq!(stored.len(), 1, "alert should be persisted");

    eprintln!("[webhook_smoke] ok: POST received and DB has 1 row");
}
