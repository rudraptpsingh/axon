use crate::alert_config::{AlertDispatchConfig, ChannelConfig};
use crate::persistence::DbHandle;
use crate::types::Alert;

// ── Webhook Payload ──────────────────────────────────────────────────────────

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookPayload {
    pub alert_type: String,
    pub severity: String,
    pub timestamp: String,
    pub message: String,
    pub metrics: WebhookMetrics,
    pub culprit: Option<WebhookCulprit>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookMetrics {
    pub ram_pct: Option<f64>,
    pub cpu_pct: Option<f64>,
    pub temp_c: Option<f64>,
    pub disk_pct: Option<f64>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WebhookCulprit {
    pub name: String,
    pub pid: u32,
    pub ram_gb: f64,
    pub cpu_pct: f64,
}

impl From<&Alert> for WebhookPayload {
    fn from(alert: &Alert) -> Self {
        let culprit = alert.metadata.culprit.as_ref().map(|c| WebhookCulprit {
            name: c.cmd.clone(),
            pid: c.pid,
            ram_gb: c.ram_gb,
            cpu_pct: c.cpu_pct,
        });
        WebhookPayload {
            alert_type: alert.alert_type.to_string(),
            severity: alert.severity.to_string(),
            timestamp: alert.ts.to_rfc3339(),
            message: alert.message.clone(),
            metrics: WebhookMetrics {
                ram_pct: alert.metadata.ram_pct,
                cpu_pct: alert.metadata.cpu_pct,
                temp_c: alert.metadata.temp_c,
                disk_pct: alert.metadata.disk_pct,
            },
            culprit,
        }
    }
}

// ── Alert Dispatcher ─────────────────────────────────────────────────────────

/// Dispatches alerts to configured webhook channels.
/// MCP channel dispatch is handled separately via the existing peer notification system.
pub struct AlertDispatcher {
    config: AlertDispatchConfig,
    http_client: reqwest::Client,
}

impl AlertDispatcher {
    pub fn new(config: AlertDispatchConfig) -> Self {
        let http_client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(5))
            .build()
            .unwrap_or_default();
        Self {
            config,
            http_client,
        }
    }

    pub fn config(&self) -> &AlertDispatchConfig {
        &self.config
    }

    /// Dispatch a single alert to all configured webhook channels (fire-and-forget).
    /// Also persists the alert to SQLite.
    /// Returns whether the alert should also be sent via MCP.
    pub async fn dispatch(&self, alert: &Alert, db: &DbHandle) -> bool {
        // Persist alert to database
        crate::persistence::insert_alert(db, alert);
        self.dispatch_webhooks_only(alert).await
    }

    /// Dispatch webhooks and return MCP flag, without inserting to DB.
    /// Use this when the caller (e.g. collector) has already persisted the alert.
    pub async fn dispatch_webhooks_only(&self, alert: &Alert) -> bool {
        let mut send_via_mcp = false;

        for channel in &self.config.channels {
            let filters = channel.filters();
            if !filters.accepts(&alert.severity, &alert.alert_type) {
                continue;
            }

            match channel {
                ChannelConfig::Mcp { .. } => {
                    send_via_mcp = true;
                }
                ChannelConfig::Webhook { url, id, .. } => {
                    let payload = WebhookPayload::from(alert);
                    let client = self.http_client.clone();
                    let url = url.clone();
                    let id = id.clone();
                    // Fire-and-forget: spawn task, don't await result
                    tokio::spawn(async move {
                        match client.post(&url).json(&payload).send().await {
                            Ok(resp) => {
                                tracing::debug!(
                                    channel = %id,
                                    status = %resp.status(),
                                    "webhook delivered"
                                );
                            }
                            Err(e) => {
                                tracing::debug!(
                                    channel = %id,
                                    error = %e,
                                    "webhook delivery failed (fire-and-forget)"
                                );
                            }
                        }
                    });
                }
            }
        }

        send_via_mcp
    }

    /// Check if any webhook channels are configured.
    pub fn has_webhooks(&self) -> bool {
        self.config
            .channels
            .iter()
            .any(|c| matches!(c, ChannelConfig::Webhook { .. }))
    }
}

// ── Convenience filter check for MCP-only mode ──────────────────────────────

/// Quick check: does the MCP channel filter accept this alert?
pub fn mcp_filter_accepts(config: &AlertDispatchConfig, alert: &Alert) -> bool {
    for channel in &config.channels {
        if let ChannelConfig::Mcp { filters, .. } = channel {
            return filters.accepts(&alert.severity, &alert.alert_type);
        }
    }
    // No MCP channel configured — don't send via MCP
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::*;

    fn make_alert(severity: AlertSeverity, alert_type: AlertType) -> Alert {
        Alert {
            severity,
            alert_type,
            message: "test alert".to_string(),
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

    #[test]
    fn test_webhook_payload_from_alert() {
        let alert = make_alert(AlertSeverity::Critical, AlertType::MemoryPressure);
        let payload = WebhookPayload::from(&alert);
        assert_eq!(payload.alert_type, "memory_pressure");
        assert_eq!(payload.severity, "critical");
        assert_eq!(payload.metrics.ram_pct, Some(85.0));
        assert!(payload.culprit.is_some());
        assert_eq!(payload.culprit.unwrap().name, "chrome");
    }

    #[test]
    fn test_webhook_payload_no_culprit() {
        let mut alert = make_alert(AlertSeverity::Warning, AlertType::ThermalThrottle);
        alert.metadata.culprit = None;
        let payload = WebhookPayload::from(&alert);
        assert!(payload.culprit.is_none());
    }

    #[test]
    fn test_webhook_payload_serializes_to_json() {
        let alert = make_alert(AlertSeverity::Critical, AlertType::MemoryPressure);
        let payload = WebhookPayload::from(&alert);
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("\"alert_type\":\"memory_pressure\""));
        assert!(json.contains("\"severity\":\"critical\""));
        assert!(json.contains("\"ram_pct\":85.0"));
    }

    #[test]
    fn test_mcp_filter_accepts_with_wildcard() {
        let config = AlertDispatchConfig::default();
        let alert = make_alert(AlertSeverity::Warning, AlertType::MemoryPressure);
        assert!(mcp_filter_accepts(&config, &alert));
    }

    #[test]
    fn test_channel_filter_severity() {
        use crate::alert_config::AlertFilters;
        let f = AlertFilters {
            severity: vec!["critical".to_string()],
            alert_types: vec![],
        };
        assert!(!f.accepts(&AlertSeverity::Warning, &AlertType::MemoryPressure));
        assert!(f.accepts(&AlertSeverity::Critical, &AlertType::MemoryPressure));
    }

    #[test]
    fn test_channel_filter_alert_type() {
        use crate::alert_config::AlertFilters;
        let f = AlertFilters {
            severity: vec![],
            alert_types: vec!["thermal_throttle".to_string()],
        };
        assert!(!f.accepts(&AlertSeverity::Critical, &AlertType::MemoryPressure));
        assert!(f.accepts(&AlertSeverity::Critical, &AlertType::ThermalThrottle));
    }

    #[test]
    fn test_channel_filter_wildcard() {
        use crate::alert_config::AlertFilters;
        let f = AlertFilters {
            severity: vec!["*".to_string()],
            alert_types: vec!["*".to_string()],
        };
        assert!(f.accepts(&AlertSeverity::Warning, &AlertType::ImpactEscalation));
    }
}
