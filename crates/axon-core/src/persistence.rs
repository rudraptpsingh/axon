use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::types::*;

pub type DbHandle = Arc<Mutex<Connection>>;

// ── Path + Open ──────────────────────────────────────────────────────────────

/// Default: `<data_local_dir>/axon/hardware.db`.
/// Override the directory with **`AXON_DATA_DIR`** (same layout: `<AXON_DATA_DIR>/hardware.db`)
/// so live tests and scripts do not share the DB with a normal Cursor session.
pub fn default_db_path() -> Result<PathBuf> {
    let data_dir = if let Some(p) = std::env::var_os("AXON_DATA_DIR") {
        PathBuf::from(p)
    } else {
        dirs::data_local_dir()
            .ok_or_else(|| anyhow::anyhow!("could not determine data directory"))?
            .join("axon")
    };
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir.join("hardware.db"))
}

pub fn open(path: PathBuf) -> Result<DbHandle> {
    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "wal")?;
    conn.pragma_update(None, "synchronous", "normal")?;
    init_schema(&conn)?;
    migrate_disk_columns(&conn)?;
    migrate_disk_pressure_column(&conn)?;
    migrate_ai_agent_count_column(&conn)?;
    prune_old_rows(&conn)?;
    Ok(Arc::new(Mutex::new(conn)))
}

fn init_schema(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS snapshots (
            id INTEGER PRIMARY KEY,
            ts TEXT NOT NULL,
            cpu_pct REAL,
            ram_used_gb REAL,
            die_temp_c REAL,
            throttling INTEGER,
            ram_pressure TEXT,
            anomaly_type TEXT,
            impact_level TEXT,
            anomaly_score REAL,
            culprit_group_name TEXT,
            disk_used_gb REAL,
            disk_total_gb REAL,
            disk_pressure TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_snapshots_ts ON snapshots(ts);
        CREATE TABLE IF NOT EXISTS alerts (
            id INTEGER PRIMARY KEY,
            ts TEXT NOT NULL,
            severity TEXT NOT NULL,
            alert_type TEXT NOT NULL,
            message TEXT NOT NULL,
            metadata_json TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_alerts_ts ON alerts(ts);
        CREATE INDEX IF NOT EXISTS idx_alerts_severity ON alerts(severity);
        CREATE INDEX IF NOT EXISTS idx_alerts_type ON alerts(alert_type);",
    )?;
    Ok(())
}

fn migrate_disk_columns(conn: &Connection) -> Result<()> {
    // Add disk columns to existing databases that lack them.
    // SQLite ALTER TABLE ADD COLUMN is a no-op if the column already exists (errors on dupe),
    // so we check first via pragma.
    let has_disk: bool = conn
        .prepare("PRAGMA table_info(snapshots)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "disk_used_gb");
    if !has_disk {
        conn.execute_batch(
            "ALTER TABLE snapshots ADD COLUMN disk_used_gb REAL;
             ALTER TABLE snapshots ADD COLUMN disk_total_gb REAL;",
        )?;
    }
    Ok(())
}

fn migrate_disk_pressure_column(conn: &Connection) -> Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(snapshots)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "disk_pressure");
    if !has_col {
        conn.execute_batch("ALTER TABLE snapshots ADD COLUMN disk_pressure TEXT;")?;
    }
    Ok(())
}

fn migrate_ai_agent_count_column(conn: &Connection) -> Result<()> {
    let has_col: bool = conn
        .prepare("PRAGMA table_info(snapshots)")?
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(|r| r.ok())
        .any(|name| name == "ai_agent_count");
    if !has_col {
        conn.execute_batch("ALTER TABLE snapshots ADD COLUMN ai_agent_count INTEGER;")?;
    }
    Ok(())
}

fn prune_old_rows(conn: &Connection) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(30);
    let cutoff_str = cutoff.to_rfc3339();
    conn.execute("DELETE FROM snapshots WHERE ts < ?1", params![&cutoff_str])?;
    conn.execute("DELETE FROM alerts WHERE ts < ?1", params![&cutoff_str])?;
    Ok(())
}

// ── Insert ───────────────────────────────────────────────────────────────────

pub fn insert_snapshot(db: &DbHandle, hw: &HwSnapshot, blame: &ProcessBlame) {
    let conn = match db.lock() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("db lock poisoned: {}", e);
            return;
        }
    };

    let group_name = blame
        .culprit_group
        .as_ref()
        .map(|g| g.name.clone())
        .or_else(|| blame.culprit.as_ref().map(|p| p.cmd.clone()));

    let ram_pressure_str = match hw.ram_pressure {
        RamPressure::Normal => "normal",
        RamPressure::Warn => "warn",
        RamPressure::Critical => "critical",
    };
    let disk_pressure_str = match hw.disk_pressure {
        DiskPressure::Normal => "normal",
        DiskPressure::Warn => "warn",
        DiskPressure::Critical => "critical",
    };
    let anomaly_str = format!("{:?}", blame.anomaly_type).to_lowercase();
    let impact_str = format!("{:?}", blame.impact_level).to_lowercase();

    if let Err(e) = conn.execute(
        "INSERT INTO snapshots (ts, cpu_pct, ram_used_gb, die_temp_c, throttling, ram_pressure, anomaly_type, impact_level, anomaly_score, culprit_group_name, disk_used_gb, disk_total_gb, disk_pressure, ai_agent_count)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            hw.ts.to_rfc3339(),
            hw.cpu_usage_pct,
            hw.ram_used_gb,
            hw.die_temp_celsius,
            hw.throttling as i32,
            ram_pressure_str,
            anomaly_str,
            impact_str,
            blame.anomaly_score,
            group_name,
            hw.disk_used_gb,
            hw.disk_total_gb,
            disk_pressure_str,
            hw.ai_agent_count as i32,
        ],
    ) {
        tracing::warn!("failed to insert snapshot: {}", e);
    }
}

/// Persist an alert to the alerts table.
pub fn insert_alert(db: &DbHandle, alert: &Alert) {
    let conn = match db.lock() {
        Ok(c) => c,
        Err(e) => {
            tracing::warn!("db lock poisoned: {}", e);
            return;
        }
    };

    let metadata_json = serde_json::to_string(&alert.metadata).unwrap_or_default();

    if let Err(e) = conn.execute(
        "INSERT INTO alerts (ts, severity, alert_type, message, metadata_json)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![
            alert.ts.to_rfc3339(),
            alert.severity.to_string(),
            alert.alert_type.to_string(),
            alert.message,
            metadata_json,
        ],
    ) {
        tracing::warn!("failed to insert alert: {}", e);
    }
}

/// Query alerts within a time range with optional filters.
/// Count rows in `alerts` (for tests and diagnostics).
pub fn count_alerts(db: &DbHandle) -> Result<u64> {
    let conn = db.lock().map_err(|e| anyhow::anyhow!("db lock: {}", e))?;
    let n: u64 = conn.query_row("SELECT COUNT(*) FROM alerts", [], |row| row.get(0))?;
    Ok(n)
}

pub fn query_alerts(
    db: &DbHandle,
    range_secs: i64,
    severity_filter: Option<&str>,
    type_filter: Option<&str>,
    limit: u32,
) -> Result<Vec<Alert>> {
    let conn = db.lock().map_err(|e| anyhow::anyhow!("db lock: {}", e))?;
    let start = Utc::now() - chrono::Duration::seconds(range_secs);
    let start_str = start.to_rfc3339();

    let mut sql =
        "SELECT ts, severity, alert_type, message, metadata_json FROM alerts WHERE ts >= ?1"
            .to_string();
    let mut param_values: Vec<String> = vec![start_str];

    if let Some(sev) = severity_filter {
        param_values.push(sev.to_string());
        sql.push_str(&format!(" AND severity = ?{}", param_values.len()));
    }
    if let Some(typ) = type_filter {
        param_values.push(typ.to_string());
        sql.push_str(&format!(" AND alert_type = ?{}", param_values.len()));
    }

    sql.push_str(&format!(" ORDER BY ts DESC LIMIT {}", limit));

    let mut stmt = conn.prepare(&sql)?;
    let params: Vec<&dyn rusqlite::ToSql> = param_values
        .iter()
        .map(|s| s as &dyn rusqlite::ToSql)
        .collect();

    let alerts = stmt
        .query_map(params.as_slice(), |row| {
            let ts_str: String = row.get(0)?;
            let severity_str: String = row.get(1)?;
            let type_str: String = row.get(2)?;
            let message: String = row.get(3)?;
            let metadata_str: String = row.get::<_, String>(4).unwrap_or_default();

            let ts = DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());

            let severity = match severity_str.as_str() {
                "critical" => AlertSeverity::Critical,
                _ => AlertSeverity::Warning,
            };

            let alert_type = match type_str.as_str() {
                "thermal_throttle" => AlertType::ThermalThrottle,
                "impact_escalation" => AlertType::ImpactEscalation,
                "disk_pressure" => AlertType::DiskPressure,
                _ => AlertType::MemoryPressure,
            };

            let metadata: AlertMetadata =
                serde_json::from_str(&metadata_str).unwrap_or(AlertMetadata {
                    ram_pct: None,
                    cpu_pct: None,
                    temp_c: None,
                    disk_pct: None,
                    culprit: None,
                    culprit_group: None,
                });

            Ok(Alert {
                severity,
                alert_type,
                message,
                ts,
                metadata,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    Ok(alerts)
}

/// Lightweight alert count query — used by the ring-buffer fast path
/// to supplement in-memory session_health with the DB alert count.
pub fn query_alert_count(db: &DbHandle, since: DateTime<Utc>) -> Result<u32> {
    let conn = db.lock().map_err(|e| anyhow::anyhow!("db lock: {}", e))?;
    let since_str = since.to_rfc3339();
    let count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM alerts WHERE ts >= ?1",
            params![since_str],
            |row| row.get(0),
        )
        .unwrap_or(0);
    Ok(count)
}

// ── Session Health ───────────────────────────────────────────────────────────

pub fn query_session_health(db: &DbHandle, since: DateTime<Utc>) -> Result<SessionHealth> {
    let conn = db.lock().map_err(|e| anyhow::anyhow!("db lock: {}", e))?;
    let since_str = since.to_rfc3339();

    // Query snapshots
    let mut stmt = conn.prepare(
        "SELECT cpu_pct, ram_used_gb, die_temp_c, throttling, anomaly_type, impact_level, anomaly_score, ai_agent_count
         FROM snapshots WHERE ts >= ?1",
    )?;

    struct SnapRow {
        cpu_pct: f64,
        ram_used_gb: f64,
        die_temp_c: Option<f64>,
        throttling: bool,
        anomaly_type: String,
        impact_level: String,
        anomaly_score: f64,
        ai_agent_count: u32,
    }

    let rows: Vec<SnapRow> = stmt
        .query_map(params![since_str], |row| {
            Ok(SnapRow {
                cpu_pct: row.get(0)?,
                ram_used_gb: row.get(1)?,
                die_temp_c: row.get(2)?,
                throttling: row.get::<_, i32>(3)? != 0,
                anomaly_type: row.get::<_, String>(4).unwrap_or_default(),
                impact_level: row.get::<_, String>(5).unwrap_or_default(),
                anomaly_score: row.get::<_, f64>(6).unwrap_or(0.0),
                ai_agent_count: row.get::<_, Option<i32>>(7)
                    .unwrap_or(None)
                    .unwrap_or(0) as u32,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    // Query alert count
    let alert_count: u32 = conn
        .query_row(
            "SELECT COUNT(*) FROM alerts WHERE ts >= ?1",
            params![since_str],
            |row| row.get(0),
        )
        .unwrap_or(0);

    if rows.is_empty() {
        return Ok(SessionHealth {
            since,
            snapshot_count: 0,
            alert_count,
            worst_impact_level: ImpactLevel::Healthy,
            worst_anomaly_type: AnomalyType::None,
            avg_anomaly_score: 0.0,
            avg_cpu_pct: 0.0,
            avg_ram_gb: 0.0,
            peak_cpu_pct: 0.0,
            peak_ram_gb: 0.0,
            peak_temp_celsius: None,
            throttle_event_count: 0,
            agent_accumulation_events: 0,
            peak_ai_agent_count: 0,
        });
    }

    let n = rows.len() as f64;
    let mut peak_cpu = 0.0_f64;
    let mut peak_ram = 0.0_f64;
    let mut peak_temp: Option<f64> = None;
    let mut sum_cpu = 0.0_f64;
    let mut sum_ram = 0.0_f64;
    let mut sum_score = 0.0_f64;
    let mut throttle_count = 0_u32;
    let mut worst_impact = 0_u8; // 0=Healthy, 1=Degrading, 2=Strained, 3=Critical
    let mut worst_anomaly = AnomalyType::None;
    let mut agent_accumulation_events = 0_u32;
    let mut peak_ai_agent_count = 0_u32;

    for row in &rows {
        sum_cpu += row.cpu_pct;
        sum_ram += row.ram_used_gb;
        sum_score += row.anomaly_score;
        if row.cpu_pct > peak_cpu {
            peak_cpu = row.cpu_pct;
        }
        if row.ram_used_gb > peak_ram {
            peak_ram = row.ram_used_gb;
        }
        if let Some(t) = row.die_temp_c {
            peak_temp = Some(peak_temp.map_or(t, |prev: f64| prev.max(t)));
        }
        if row.throttling {
            throttle_count += 1;
        }

        if matches!(row.anomaly_type.as_str(), "agent_accumulation" | "agentaccumulation") {
            agent_accumulation_events += 1;
        }
        if row.ai_agent_count > peak_ai_agent_count {
            peak_ai_agent_count = row.ai_agent_count;
        }

        let level_ord = match row.impact_level.as_str() {
            "degrading" => 1,
            "strained" => 2,
            "critical" => 3,
            _ => 0,
        };
        if level_ord > worst_impact {
            worst_impact = level_ord;
        }

        let anomaly_ord = match row.anomaly_type.as_str() {
            "general_slowdown" | "generalslowdown" => 1,
            "memory_pressure" | "memorypressure" => 2,
            "cpu_saturation" | "cpusaturation" => 3,
            "thermal_throttle" | "thermalthrottle" => 4,
            "agent_accumulation" | "agentaccumulation" => 5,
            _ => 0,
        };
        let current_worst_ord = match worst_anomaly {
            AnomalyType::None => 0,
            AnomalyType::GeneralSlowdown => 1,
            AnomalyType::MemoryPressure => 2,
            AnomalyType::CpuSaturation => 3,
            AnomalyType::ThermalThrottle => 4,
            AnomalyType::AgentAccumulation => 5,
        };
        if anomaly_ord > current_worst_ord {
            worst_anomaly = match row.anomaly_type.as_str() {
                "general_slowdown" | "generalslowdown" => AnomalyType::GeneralSlowdown,
                "memory_pressure" | "memorypressure" => AnomalyType::MemoryPressure,
                "cpu_saturation" | "cpusaturation" => AnomalyType::CpuSaturation,
                "thermal_throttle" | "thermalthrottle" => AnomalyType::ThermalThrottle,
                "agent_accumulation" | "agentaccumulation" => AnomalyType::AgentAccumulation,
                _ => AnomalyType::None,
            };
        }
    }

    let worst_impact_level = match worst_impact {
        1 => ImpactLevel::Degrading,
        2 => ImpactLevel::Strained,
        3 => ImpactLevel::Critical,
        _ => ImpactLevel::Healthy,
    };

    Ok(SessionHealth {
        since,
        snapshot_count: rows.len() as u32,
        alert_count,
        worst_impact_level,
        worst_anomaly_type: worst_anomaly,
        avg_anomaly_score: sum_score / n,
        avg_cpu_pct: sum_cpu / n,
        avg_ram_gb: sum_ram / n,
        peak_cpu_pct: peak_cpu,
        peak_ram_gb: peak_ram,
        peak_temp_celsius: peak_temp,
        throttle_event_count: throttle_count,
        agent_accumulation_events,
        peak_ai_agent_count,
    })
}

// ── Query ────────────────────────────────────────────────────────────────────

pub fn parse_time_range(s: &str) -> Option<i64> {
    match s {
        "last_1h" => Some(3600),
        "last_6h" => Some(3600 * 6),
        "last_24h" => Some(3600 * 24),
        "last_7d" => Some(3600 * 24 * 7),
        "last_30d" => Some(3600 * 24 * 30),
        _ => None,
    }
}

pub fn parse_interval(s: &str) -> Option<i64> {
    match s {
        "1m" => Some(60),
        "5m" => Some(300),
        "15m" => Some(900),
        "1h" => Some(3600),
        "1d" => Some(86400),
        _ => None,
    }
}

pub fn query_trend(db: &DbHandle, range_secs: i64, bucket_secs: i64) -> Result<TrendData> {
    let conn = db.lock().map_err(|e| anyhow::anyhow!("db lock: {}", e))?;

    let start = Utc::now() - chrono::Duration::seconds(range_secs);
    let start_str = start.to_rfc3339();

    let mut stmt = conn.prepare(
        "SELECT ts, cpu_pct, ram_used_gb, die_temp_c, throttling, anomaly_type
         FROM snapshots
         WHERE ts >= ?1
         ORDER BY ts ASC",
    )?;

    struct Row {
        ts: DateTime<Utc>,
        cpu_pct: f64,
        ram_used_gb: f64,
        die_temp_c: Option<f64>,
        throttling: bool,
        anomaly_type: String,
    }

    let rows: Vec<Row> = stmt
        .query_map(params![start_str], |row| {
            let ts_str: String = row.get(0)?;
            let ts = DateTime::parse_from_rfc3339(&ts_str)
                .map(|dt| dt.with_timezone(&Utc))
                .unwrap_or_else(|_| Utc::now());
            Ok(Row {
                ts,
                cpu_pct: row.get(1)?,
                ram_used_gb: row.get(2)?,
                die_temp_c: row.get(3)?,
                throttling: row.get::<_, i32>(4)? != 0,
                anomaly_type: row.get(5)?,
            })
        })?
        .filter_map(|r| r.ok())
        .collect();

    if rows.is_empty() {
        return Ok(TrendData {
            buckets: vec![],
            trend_direction: "insufficient_data".to_string(),
            total_snapshots: 0,
        });
    }

    // Bucket the rows
    let first_ts = rows[0].ts.timestamp();
    let mut bucket_map: std::collections::BTreeMap<i64, Vec<&Row>> =
        std::collections::BTreeMap::new();

    for row in &rows {
        let offset = row.ts.timestamp() - first_ts;
        let bucket_idx = offset / bucket_secs;
        bucket_map.entry(bucket_idx).or_default().push(row);
    }

    let mut buckets = Vec::new();
    for (&idx, bucket_rows) in &bucket_map {
        let bucket_start = first_ts + idx * bucket_secs;
        let n = bucket_rows.len() as f64;

        let cpu_vals: Vec<f64> = bucket_rows.iter().map(|r| r.cpu_pct).collect();
        let ram_vals: Vec<f64> = bucket_rows.iter().map(|r| r.ram_used_gb).collect();
        let temp_vals: Vec<f64> = bucket_rows.iter().filter_map(|r| r.die_temp_c).collect();

        let anomaly_count = bucket_rows
            .iter()
            .filter(|r| r.anomaly_type != "none")
            .count();
        let throttle_count = bucket_rows.iter().filter(|r| r.throttling).count();

        buckets.push(TrendBucket {
            bucket_start: DateTime::from_timestamp(bucket_start, 0).unwrap_or_else(Utc::now),
            sample_count: bucket_rows.len() as u32,
            avg_cpu_pct: cpu_vals.iter().sum::<f64>() / n,
            peak_cpu_pct: cpu_vals.iter().cloned().fold(f64::MIN, f64::max),
            avg_ram_gb: ram_vals.iter().sum::<f64>() / n,
            peak_ram_gb: ram_vals.iter().cloned().fold(f64::MIN, f64::max),
            avg_temp_celsius: if temp_vals.is_empty() {
                None
            } else {
                Some(temp_vals.iter().sum::<f64>() / temp_vals.len() as f64)
            },
            peak_temp_celsius: if temp_vals.is_empty() {
                None
            } else {
                Some(temp_vals.iter().cloned().fold(f64::MIN, f64::max))
            },
            anomaly_count: anomaly_count as u32,
            throttle_count: throttle_count as u32,
        });
    }

    // Trend direction: compare first-half avg CPU to second-half avg CPU
    let trend_direction = if buckets.len() < 2 {
        "stable".to_string()
    } else {
        let mid = buckets.len() / 2;
        let first_avg: f64 = buckets[..mid].iter().map(|b| b.avg_cpu_pct).sum::<f64>() / mid as f64;
        let second_avg: f64 =
            buckets[mid..].iter().map(|b| b.avg_cpu_pct).sum::<f64>() / (buckets.len() - mid) as f64;
        let delta = second_avg - first_avg;
        if delta > 5.0 {
            "rising".to_string()
        } else if delta < -5.0 {
            "falling".to_string()
        } else {
            "stable".to_string()
        }
    };

    Ok(TrendData {
        total_snapshots: rows.len() as u32,
        buckets,
        trend_direction,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::NamedTempFile;

    fn test_db() -> DbHandle {
        let tmp = NamedTempFile::new().unwrap();
        open(tmp.path().to_path_buf()).unwrap()
    }

    fn sample_hw(cpu: f64, ram: f64, temp: Option<f64>) -> HwSnapshot {
        HwSnapshot {
            die_temp_celsius: temp,
            throttling: false,
            ram_used_gb: ram,
            ram_total_gb: 8.0,
            ram_pressure: RamPressure::Normal,
            cpu_usage_pct: cpu,
            disk_used_gb: 250.0,
            disk_total_gb: 500.0,
            disk_pressure: DiskPressure::Normal,
            headroom: HeadroomLevel::Adequate,
            headroom_reason: "System has headroom".to_string(),
            ts: Utc::now(),
            cpu_trend: TrendDirection::Stable,
            ram_trend: TrendDirection::Stable,
            temp_trend: TrendDirection::Stable,
            cpu_delta_pct: 0.0,
            ram_delta_gb: 0.0,
            top_culprit: String::new(),
            impact_level: ImpactLevel::Healthy,
            impact_duration_s: 0,
            one_liner: String::new(),
            ai_agent_count: 0,
            ai_agent_ram_gb: 0.0,
                swap_used_gb: None,
                swap_total_gb: None,
                disk_fill_rate_gb_per_sec: None,
            irq_per_sec: None,
        }
    }

    fn sample_blame() -> ProcessBlame {
        ProcessBlame {
            anomaly_type: AnomalyType::None,
            impact_level: ImpactLevel::Healthy,
            culprit: None,
            culprit_group: None,
            anomaly_score: 0.0,
            impact: String::new(),
            fix: String::new(),
            ts: Utc::now(),
            stale_axon_pids: Vec::new(),
            urgency: Urgency::Monitor,
            culprit_category: CulpritCategory::Unknown,
            claude_agents: Vec::new(),
            stranded_idle_pids: Vec::new(),
            orphan_pids: Vec::new(),
            zombie_pids: Vec::new(),
            crashed_agent_pids: Vec::new(),
        }
    }

    #[test]
    fn test_open_creates_schema() {
        let db = test_db();
        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_insert_and_count() {
        let db = test_db();
        let hw = sample_hw(50.0, 4.0, Some(65.0));
        let blame = sample_blame();

        insert_snapshot(&db, &hw, &blame);
        insert_snapshot(&db, &hw, &blame);

        let conn = db.lock().unwrap();
        let count: i64 = conn
            .query_row("SELECT COUNT(*) FROM snapshots", [], |r| r.get(0))
            .unwrap();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_insert_with_culprit_group() {
        let db = test_db();
        let hw = sample_hw(90.0, 6.0, Some(85.0));
        let blame = ProcessBlame {
            culprit_group: Some(ProcessGroup {
                name: "Chrome".to_string(),
                process_count: 12,
                total_cpu_pct: 150.0,
                total_ram_gb: 4.5,
                blame_score: 0.8,
                top_pid: 1234,
                pids: vec![],
            }),
            ..sample_blame()
        };

        insert_snapshot(&db, &hw, &blame);

        let conn = db.lock().unwrap();
        let name: String = conn
            .query_row(
                "SELECT culprit_group_name FROM snapshots LIMIT 1",
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(name, "Chrome");
    }

    #[test]
    fn test_query_trend_empty_db() {
        let db = test_db();
        let trend = query_trend(&db, 3600, 900).unwrap();
        assert_eq!(trend.total_snapshots, 0);
        assert_eq!(trend.trend_direction, "insufficient_data");
        assert!(trend.buckets.is_empty());
    }

    #[test]
    fn test_query_trend_with_data() {
        let db = test_db();
        let blame = sample_blame();

        // Insert 5 snapshots
        for cpu in [20.0, 30.0, 40.0, 50.0, 60.0] {
            let hw = sample_hw(cpu, 4.0, Some(65.0));
            insert_snapshot(&db, &hw, &blame);
        }

        let trend = query_trend(&db, 3600, 900).unwrap();
        assert_eq!(trend.total_snapshots, 5);
        assert!(!trend.buckets.is_empty());
        // All in one bucket since timestamps are nearly identical
        assert_eq!(trend.buckets[0].sample_count, 5);
        assert!((trend.buckets[0].avg_cpu_pct - 40.0).abs() < 0.1);
        assert!((trend.buckets[0].peak_cpu_pct - 60.0).abs() < 0.1);
    }

    #[test]
    fn test_query_trend_no_temp() {
        let db = test_db();
        let hw = sample_hw(50.0, 4.0, None);
        insert_snapshot(&db, &hw, &sample_blame());

        let trend = query_trend(&db, 3600, 900).unwrap();
        assert!(trend.buckets[0].avg_temp_celsius.is_none());
        assert!(trend.buckets[0].peak_temp_celsius.is_none());
    }

    #[test]
    fn test_parse_time_range() {
        assert_eq!(parse_time_range("last_1h"), Some(3600));
        assert_eq!(parse_time_range("last_24h"), Some(86400));
        assert_eq!(parse_time_range("last_7d"), Some(604800));
        assert_eq!(parse_time_range("invalid"), None);
    }

    #[test]
    fn test_parse_interval() {
        assert_eq!(parse_interval("1m"), Some(60));
        assert_eq!(parse_interval("15m"), Some(900));
        assert_eq!(parse_interval("1h"), Some(3600));
        assert_eq!(parse_interval("1d"), Some(86400));
        assert_eq!(parse_interval("bad"), None);
    }

    #[test]
    fn test_default_db_path() {
        let path = default_db_path().unwrap();
        assert!(path.to_string_lossy().contains("axon"));
        assert!(path.to_string_lossy().ends_with("hardware.db"));
    }
}
