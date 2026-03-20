use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{params, Connection};

use crate::types::*;

pub type DbHandle = Arc<Mutex<Connection>>;

// ── Path + Open ──────────────────────────────────────────────────────────────

pub fn default_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .ok_or_else(|| anyhow::anyhow!("could not determine data directory"))?
        .join("axon");
    std::fs::create_dir_all(&data_dir)?;
    Ok(data_dir.join("hardware.db"))
}

pub fn open(path: PathBuf) -> Result<DbHandle> {
    let conn = Connection::open(&path)?;
    conn.pragma_update(None, "journal_mode", "wal")?;
    conn.pragma_update(None, "synchronous", "normal")?;
    init_schema(&conn)?;
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
            culprit_group_name TEXT
        );
        CREATE INDEX IF NOT EXISTS idx_snapshots_ts ON snapshots(ts);",
    )?;
    Ok(())
}

fn prune_old_rows(conn: &Connection) -> Result<()> {
    let cutoff = Utc::now() - chrono::Duration::days(30);
    conn.execute(
        "DELETE FROM snapshots WHERE ts < ?1",
        params![cutoff.to_rfc3339()],
    )?;
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
    let anomaly_str = format!("{:?}", blame.anomaly_type).to_lowercase();
    let impact_str = format!("{:?}", blame.impact_level).to_lowercase();

    if let Err(e) = conn.execute(
        "INSERT INTO snapshots (ts, cpu_pct, ram_used_gb, die_temp_c, throttling, ram_pressure, anomaly_type, impact_level, anomaly_score, culprit_group_name)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
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
        ],
    ) {
        tracing::warn!("failed to insert snapshot: {}", e);
    }
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

pub fn query_trend(
    db: &DbHandle,
    range_secs: i64,
    bucket_secs: i64,
) -> Result<TrendData> {
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
            bucket_start: DateTime::from_timestamp(bucket_start, 0)
                .unwrap_or_else(|| Utc::now()),
            sample_count: bucket_rows.len() as u32,
            cpu_avg: cpu_vals.iter().sum::<f64>() / n,
            cpu_max: cpu_vals.iter().cloned().fold(f64::MIN, f64::max),
            ram_avg: ram_vals.iter().sum::<f64>() / n,
            ram_max: ram_vals.iter().cloned().fold(f64::MIN, f64::max),
            temp_avg: if temp_vals.is_empty() {
                None
            } else {
                Some(temp_vals.iter().sum::<f64>() / temp_vals.len() as f64)
            },
            temp_max: if temp_vals.is_empty() {
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
        let first_avg: f64 =
            buckets[..mid].iter().map(|b| b.cpu_avg).sum::<f64>() / mid as f64;
        let second_avg: f64 =
            buckets[mid..].iter().map(|b| b.cpu_avg).sum::<f64>() / (buckets.len() - mid) as f64;
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
            ts: Utc::now(),
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
        assert!((trend.buckets[0].cpu_avg - 40.0).abs() < 0.1);
        assert!((trend.buckets[0].cpu_max - 60.0).abs() < 0.1);
    }

    #[test]
    fn test_query_trend_no_temp() {
        let db = test_db();
        let hw = sample_hw(50.0, 4.0, None);
        insert_snapshot(&db, &hw, &sample_blame());

        let trend = query_trend(&db, 3600, 900).unwrap();
        assert!(trend.buckets[0].temp_avg.is_none());
        assert!(trend.buckets[0].temp_max.is_none());
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
