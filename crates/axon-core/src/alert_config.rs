use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::types::{AlertSeverity, AlertType};

// ── Configuration Types ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AlertDispatchConfig {
    pub channels: Vec<ChannelConfig>,
    /// Optional threshold overrides. Unset fields use compiled-in defaults.
    #[serde(default)]
    pub thresholds: Option<crate::thresholds::ThresholdOverrides>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ChannelConfig {
    Mcp {
        id: String,
        #[serde(default)]
        filters: AlertFilters,
    },
    Webhook {
        id: String,
        url: String,
        #[serde(default)]
        filters: AlertFilters,
    },
}

impl ChannelConfig {
    pub fn id(&self) -> &str {
        match self {
            ChannelConfig::Mcp { id, .. } => id,
            ChannelConfig::Webhook { id, .. } => id,
        }
    }

    pub fn filters(&self) -> &AlertFilters {
        match self {
            ChannelConfig::Mcp { filters, .. } => filters,
            ChannelConfig::Webhook { filters, .. } => filters,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AlertFilters {
    /// Severity levels to accept. Empty or ["*"] means all.
    #[serde(default)]
    pub severity: Vec<String>,

    /// Alert types to accept. Empty or ["*"] means all.
    #[serde(default)]
    pub alert_types: Vec<String>,
}

impl AlertFilters {
    /// Check whether a given alert passes this filter.
    pub fn accepts(&self, severity: &AlertSeverity, alert_type: &AlertType) -> bool {
        let sev_ok = self.severity.is_empty()
            || self.severity.iter().any(|s| s == "*")
            || self.severity.iter().any(|s| s == &severity.to_string());

        let type_ok = self.alert_types.is_empty()
            || self.alert_types.iter().any(|s| s == "*")
            || self
                .alert_types
                .iter()
                .any(|s| s == &alert_type.to_string());

        sev_ok && type_ok
    }
}

impl Default for AlertDispatchConfig {
    fn default() -> Self {
        Self {
            channels: vec![ChannelConfig::Mcp {
                id: "mcp_client".to_string(),
                filters: AlertFilters {
                    severity: vec![],
                    alert_types: vec!["*".to_string()],
                },
            }],
            thresholds: None,
        }
    }
}

// ── Config Loading ───────────────────────────────────────────────────────────

/// Default config directory path.
pub fn default_config_dir() -> Option<PathBuf> {
    dirs::config_dir().map(|d| d.join("axon"))
}

/// Load config from `~/.config/axon/alert-dispatch.json`.
/// If `AXON_CONFIG_DIR` is set to a directory path, loads `<dir>/alert-dispatch.json` instead
/// (CLI override for tests and scripted runs). Caller `config_dir` wins over the env var.
/// Returns default (MCP-only) config if file doesn't exist.
pub fn load_config(config_dir: Option<&PathBuf>) -> AlertDispatchConfig {
    let path = config_dir
        .cloned()
        .or_else(|| std::env::var_os("AXON_CONFIG_DIR").map(PathBuf::from))
        .or_else(default_config_dir)
        .map(|d| d.join("alert-dispatch.json"));

    let Some(path) = path else {
        return AlertDispatchConfig::default();
    };

    match std::fs::read_to_string(&path) {
        Ok(contents) => match serde_json::from_str::<AlertDispatchConfig>(&contents) {
            Ok(config) => match validate_dispatch_config(&config) {
                Ok(()) => {
                    // Initialize threshold overrides from config file
                    if let Some(overrides) = &config.thresholds {
                        crate::thresholds::init_overrides(overrides.clone());
                    }
                    config
                }
                Err(e) => {
                    tracing::warn!("invalid alert config {}: {}", path.display(), e);
                    AlertDispatchConfig::default()
                }
            },
            Err(e) => {
                tracing::warn!("failed to parse alert config {}: {}", path.display(), e);
                AlertDispatchConfig::default()
            }
        },
        Err(_) => AlertDispatchConfig::default(),
    }
}

/// Ensure webhook channel URLs are usable before dispatch.
pub fn validate_dispatch_config(config: &AlertDispatchConfig) -> Result<(), String> {
    for c in &config.channels {
        if let ChannelConfig::Webhook { url, .. } = c {
            crate::webhooks::validate_webhook_url(url)?;
        }
    }
    Ok(())
}

/// Parse `--alert-webhook myapp=http://host/path` (first `=` splits id and URL).
pub fn parse_alert_webhook_flag(s: &str) -> Result<(String, String), String> {
    let (id, url) = s
        .split_once('=')
        .ok_or_else(|| "expected ID=URL (e.g. myapp=http://127.0.0.1:3000/hook)".to_string())?;
    if id.is_empty() {
        return Err("webhook id must not be empty".to_string());
    }
    crate::webhooks::validate_webhook_url(url)?;
    Ok((id.to_string(), url.to_string()))
}

/// Parse `--alert-filter myapp.severity=critical` into (channel_id, key, value).
pub fn parse_alert_filter_flag(s: &str) -> Result<(String, String, String), String> {
    let (left, value) = s
        .split_once('=')
        .ok_or_else(|| "expected CHANNEL.KEY=value (e.g. myapp.severity=critical)".to_string())?;
    let dot = left
        .rfind('.')
        .ok_or_else(|| "expected CHANNEL.KEY=value".to_string())?;
    let channel = left[..dot].to_string();
    let key = left[dot + 1..].to_string();
    if channel.is_empty() || key.is_empty() {
        return Err("channel and key must not be empty".to_string());
    }
    Ok((channel, key, value.to_string()))
}

/// Parse CLI `--alert-webhook id=url` flags into channel configs that override file config.
pub fn apply_cli_overrides(
    mut config: AlertDispatchConfig,
    webhooks: &[(String, String)],
    filters: &[(String, String, String)],
) -> AlertDispatchConfig {
    for (id, url) in webhooks {
        // Remove existing channel with same id
        config.channels.retain(|c| c.id() != id);
        config.channels.push(ChannelConfig::Webhook {
            id: id.clone(),
            url: url.clone(),
            filters: AlertFilters::default(),
        });
    }

    // Apply filters: (channel_id, key, value)  e.g. ("myapp", "severity", "critical")
    for (channel_id, key, value) in filters {
        for channel in &mut config.channels {
            if channel.id() == channel_id {
                let f = match channel {
                    ChannelConfig::Mcp { filters, .. } => filters,
                    ChannelConfig::Webhook { filters, .. } => filters,
                };
                match key.as_str() {
                    "severity" => {
                        f.severity = value.split(',').map(|s| s.trim().to_string()).collect();
                    }
                    "types" => {
                        f.alert_types = value.split(',').map(|s| s.trim().to_string()).collect();
                    }
                    _ => {
                        tracing::warn!("unknown filter key: {}", key);
                    }
                }
            }
        }
    }

    config
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn test_filter_accepts_all_by_default() {
        let f = AlertFilters::default();
        assert!(f.accepts(&AlertSeverity::Warning, &AlertType::MemoryPressure));
        assert!(f.accepts(&AlertSeverity::Critical, &AlertType::ThermalThrottle));
    }

    #[test]
    fn test_channel_filter_wildcard() {
        let f = AlertFilters {
            severity: vec!["*".to_string()],
            alert_types: vec!["*".to_string()],
        };
        assert!(f.accepts(&AlertSeverity::Warning, &AlertType::MemoryPressure));
    }

    #[test]
    fn test_channel_filter_severity() {
        let f = AlertFilters {
            severity: vec!["critical".to_string()],
            alert_types: vec![],
        };
        assert!(!f.accepts(&AlertSeverity::Warning, &AlertType::MemoryPressure));
        assert!(f.accepts(&AlertSeverity::Critical, &AlertType::MemoryPressure));
    }

    #[test]
    fn test_channel_filter_alert_type() {
        let f = AlertFilters {
            severity: vec![],
            alert_types: vec!["thermal_throttle".to_string()],
        };
        assert!(!f.accepts(&AlertSeverity::Critical, &AlertType::MemoryPressure));
        assert!(f.accepts(&AlertSeverity::Critical, &AlertType::ThermalThrottle));
    }

    #[test]
    fn test_default_config_has_mcp_channel() {
        let config = AlertDispatchConfig::default();
        assert_eq!(config.channels.len(), 1);
        assert_eq!(config.channels[0].id(), "mcp_client");
    }

    #[test]
    fn test_config_parse_missing_file() {
        let config = load_config(Some(&PathBuf::from("/nonexistent/dir")));
        assert_eq!(config.channels.len(), 1);
        assert_eq!(config.channels[0].id(), "mcp_client");
    }

    #[test]
    fn test_cli_override_adds_webhook() {
        let config = AlertDispatchConfig::default();
        let config = apply_cli_overrides(
            config,
            &[(
                "openclaw".to_string(),
                "http://localhost:3000/alerts".to_string(),
            )],
            &[],
        );
        assert_eq!(config.channels.len(), 2);
        assert_eq!(config.channels[1].id(), "openclaw");
    }

    #[test]
    fn test_config_cli_override_merges() {
        let config = AlertDispatchConfig {
            channels: vec![ChannelConfig::Webhook {
                id: "test".to_string(),
                url: "http://127.0.0.1:2/old".to_string(),
                filters: AlertFilters::default(),
            }],
            thresholds: None,
        };
        let config = apply_cli_overrides(
            config,
            &[("test".to_string(), "http://127.0.0.1:3/new".to_string())],
            &[],
        );
        assert_eq!(config.channels.len(), 1);
        match &config.channels[0] {
            ChannelConfig::Webhook { url, .. } => assert_eq!(url, "http://127.0.0.1:3/new"),
            _ => panic!("expected webhook"),
        }
    }

    #[test]
    fn test_cli_filter_applied() {
        let config = AlertDispatchConfig::default();
        let config = apply_cli_overrides(
            config,
            &[("myapp".to_string(), "http://localhost:9000".to_string())],
            &[
                (
                    "myapp".to_string(),
                    "severity".to_string(),
                    "critical".to_string(),
                ),
                (
                    "myapp".to_string(),
                    "types".to_string(),
                    "thermal_throttle,memory_pressure".to_string(),
                ),
            ],
        );
        let ch = &config.channels[1];
        let f = ch.filters();
        assert_eq!(f.severity, vec!["critical"]);
        assert_eq!(f.alert_types, vec!["thermal_throttle", "memory_pressure"]);
    }

    #[test]
    fn test_config_parse_valid() {
        let tmp = tempfile::tempdir().unwrap();
        let cfg = AlertDispatchConfig {
            channels: vec![
                ChannelConfig::Mcp {
                    id: "m".to_string(),
                    filters: AlertFilters::default(),
                },
                ChannelConfig::Webhook {
                    id: "w".to_string(),
                    url: "http://127.0.0.1:1/h".to_string(),
                    filters: AlertFilters::default(),
                },
            ],
            thresholds: None,
        };
        let path = tmp.path().join("alert-dispatch.json");
        std::fs::write(&path, serde_json::to_string(&cfg).unwrap()).unwrap();
        let loaded = load_config(Some(&tmp.path().to_path_buf()));
        assert_eq!(loaded.channels.len(), 2);
        assert_eq!(loaded.channels[1].id(), "w");
    }

    #[test]
    fn test_config_parse_empty_channels() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("alert-dispatch.json");
        std::fs::write(&path, r#"{"channels":[]}"#).unwrap();
        let loaded = load_config(Some(&tmp.path().to_path_buf()));
        assert!(loaded.channels.is_empty());
    }

    #[test]
    fn test_config_invalid_webhook_url_in_file_falls_back_to_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("alert-dispatch.json");
        std::fs::write(
            &path,
            r#"{"channels":[{"type":"webhook","id":"bad","url":"not-a-valid-url","filters":{}}]}"#,
        )
        .unwrap();
        let loaded = load_config(Some(&tmp.path().to_path_buf()));
        assert_eq!(loaded.channels.len(), 1);
        assert_eq!(loaded.channels[0].id(), "mcp_client");
    }

    #[test]
    fn test_parse_alert_webhook_flag() {
        let (id, u) = parse_alert_webhook_flag("myapp=http://localhost:3000/alerts").unwrap();
        assert_eq!(id, "myapp");
        assert_eq!(u, "http://localhost:3000/alerts");
        assert!(parse_alert_webhook_flag("bad").is_err());
    }

    #[test]
    fn test_parse_alert_filter_flag() {
        let (c, k, v) = parse_alert_filter_flag("myapp.severity=critical").unwrap();
        assert_eq!(c, "myapp");
        assert_eq!(k, "severity");
        assert_eq!(v, "critical");
    }
}
