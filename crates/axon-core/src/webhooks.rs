//! Webhook URL validation and payload documentation.
//! HTTP delivery is implemented in [`crate::alert_dispatch::AlertDispatcher`].

pub use crate::alert_dispatch::WebhookPayload;

/// Reject malformed or unsupported webhook URLs before dispatch.
pub fn validate_webhook_url(url: &str) -> Result<(), String> {
    let u = reqwest::Url::parse(url).map_err(|e| e.to_string())?;
    match u.scheme() {
        "http" | "https" => {}
        s => return Err(format!("unsupported URL scheme: {s} (use http or https)")),
    }
    match u.host_str() {
        None | Some("") => return Err("URL must include a host".to_string()),
        Some(_) => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_webhook_payload_serialization() {
        let json = serde_json::json!({
            "alert_type": "memory_pressure",
            "severity": "critical",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": "test",
            "metrics": { "ram_pct": 85.0, "cpu_pct": 72.0, "temp_c": 65.0 },
            "culprit": { "name": "chrome", "pid": 1, "ram_gb": 2.0, "cpu_pct": 10.0 }
        });
        let p: WebhookPayload = serde_json::from_value(json.clone()).expect("parse");
        assert_eq!(p.alert_type, "memory_pressure");
        assert_eq!(p.severity, "critical");
        assert!(p.message.contains("test"));
        assert_eq!(p.metrics.ram_pct, Some(85.0));
        assert!(p.culprit.is_some());
        let out = serde_json::to_value(&p).unwrap();
        assert!(out.get("alert_type").is_some());
        assert!(out.get("metrics").is_some());
    }

    #[test]
    fn test_webhook_payload_missing_culprit() {
        let json = serde_json::json!({
            "alert_type": "impact_escalation",
            "severity": "warning",
            "timestamp": "2026-01-01T00:00:00Z",
            "message": "msg",
            "metrics": { "ram_pct": null, "cpu_pct": null, "temp_c": null },
            "culprit": null
        });
        let p: WebhookPayload = serde_json::from_value(json).unwrap();
        assert!(p.culprit.is_none());
    }

    #[test]
    fn test_webhook_url_validation() {
        assert!(validate_webhook_url("http://127.0.0.1:9/path").is_ok());
        assert!(validate_webhook_url("https://example.com/hook").is_ok());
        assert!(validate_webhook_url("not-a-url").is_err());
        assert!(validate_webhook_url("ftp://example.com/").is_err());
        assert!(validate_webhook_url("http://").is_err());
    }
}
