//! GPU metrics collector.
//!
//! macOS: Queries the IOAccelerator kernel object via `ioreg` and parses the
//! `PerformanceStatistics` dictionary.  No sudo required.  Covers Apple Silicon
//! (AGXAccelerator) and Intel discrete/integrated GPUs (IOAccelerator subclasses).
//!
//! Linux / other: Returns `None` for all live fields; only static identity from
//! sysinfo is available.

use crate::types::GpuSnapshot;

/// Collect a GPU snapshot.  Cheap to call every collector tick (~5 ms on M-series).
pub fn read_gpu_snapshot() -> GpuSnapshot {
    #[cfg(target_os = "macos")]
    {
        read_gpu_macos()
    }
    #[cfg(not(target_os = "macos"))]
    {
        GpuSnapshot {
            utilization_pct: None,
            tiler_utilization_pct: None,
            renderer_utilization_pct: None,
            vram_used_bytes: None,
            vram_alloc_bytes: None,
            recovery_count: None,
            model: None,
            core_count: None,
            ts: chrono::Utc::now(),
        }
    }
}

// ── macOS implementation ───────────────────────────────────────────────────────

#[cfg(target_os = "macos")]
fn read_gpu_macos() -> GpuSnapshot {
    use std::process::Command;

    let now = chrono::Utc::now();

    let output = match Command::new("ioreg")
        .args(["-r", "-c", "IOAccelerator", "-d", "1", "-w", "0"])
        .output()
    {
        Ok(o) => o,
        Err(_) => return empty_snapshot(now),
    };

    if !output.status.success() {
        return empty_snapshot(now);
    }

    let text = String::from_utf8_lossy(&output.stdout);
    parse_ioreg_accelerator(&text, now)
}

#[cfg(target_os = "macos")]
fn empty_snapshot(ts: chrono::DateTime<chrono::Utc>) -> GpuSnapshot {
    GpuSnapshot {
        utilization_pct: None,
        tiler_utilization_pct: None,
        renderer_utilization_pct: None,
        vram_used_bytes: None,
        vram_alloc_bytes: None,
        recovery_count: None,
        model: None,
        core_count: None,
        ts,
    }
}

/// Parse the text output of `ioreg -r -c IOAccelerator -d 1 -w 0`.
///
/// The output contains a node with a `PerformanceStatistics` dictionary like:
/// ```text
/// "PerformanceStatistics" = {"Device Utilization %"=16,"Tiler Utilization %"=16,...}
/// ```
/// We extract key fields without pulling in a JSON/plist parser.
#[cfg(target_os = "macos")]
fn parse_ioreg_accelerator(text: &str, ts: chrono::DateTime<chrono::Utc>) -> GpuSnapshot {
    // Extract the PerformanceStatistics value (everything between the outermost { })
    let perf_stats = extract_perf_stats(text);

    let utilization_pct = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "Device Utilization %"))
        .map(|v| v as f64);

    let tiler_utilization_pct = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "Tiler Utilization %"))
        .map(|v| v as f64);

    let renderer_utilization_pct = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "Renderer Utilization %"))
        .map(|v| v as f64);

    let vram_used_bytes = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "In use system memory"))
        .map(|v| v as u64);

    let vram_alloc_bytes = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "Alloc system memory"))
        .map(|v| v as u64);

    let recovery_count = perf_stats
        .as_deref()
        .and_then(|s| extract_int(s, "recoveryCount"))
        .map(|v| v as u64);

    // model and core_count come from top-level node properties
    let model = extract_quoted_string(text, "model");
    let core_count = extract_int(text, "gpu-core-count").map(|v| v as u32);

    GpuSnapshot {
        utilization_pct,
        tiler_utilization_pct,
        renderer_utilization_pct,
        vram_used_bytes,
        vram_alloc_bytes,
        recovery_count,
        model,
        core_count,
        ts,
    }
}

/// Extract the content of the `PerformanceStatistics` dict value.
/// The ioreg line looks like:
///   `  "PerformanceStatistics" = {"Device Utilization %"=16,...}`
/// We return everything inside the outermost `{…}`.
#[cfg(target_os = "macos")]
fn extract_perf_stats(text: &str) -> Option<String> {
    let marker = "\"PerformanceStatistics\" = {";
    let start = text.find(marker)?;
    let brace_start = start + marker.len() - 1; // points at '{'
    // Walk forward tracking brace depth to find the matching '}'
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut end = brace_start;
    for (i, &b) in bytes[brace_start..].iter().enumerate() {
        match b {
            b'{' => depth += 1,
            b'}' => {
                depth -= 1;
                if depth == 0 {
                    end = brace_start + i;
                    break;
                }
            }
            _ => {}
        }
    }
    if depth != 0 {
        return None;
    }
    // Return the inner content (excluding the outer braces)
    Some(text[brace_start + 1..end].to_string())
}

/// Extract an integer value for a key from ioreg dict text.
/// Matches patterns like: `"Device Utilization %"=16` or `gpu-core-count = 10`
#[cfg(target_os = "macos")]
fn extract_int(text: &str, key: &str) -> Option<i64> {
    // Try quoted key: `"key"=VALUE` or `"key" = VALUE`
    let quoted_pattern = format!("\"{}\"", key);
    if let Some(pos) = text.find(&quoted_pattern) {
        let after = text[pos + quoted_pattern.len()..].trim_start_matches(|c: char| c == ' ');
        if let Some(rest) = after.strip_prefix('=') {
            let rest = rest.trim_start();
            return parse_leading_int(rest);
        }
    }
    // Try unquoted key: `key = VALUE` or `key=VALUE`
    let unquoted = format!("{} =", key);
    if let Some(pos) = text.find(&unquoted) {
        let after = &text[pos + unquoted.len()..];
        let rest = after.trim_start();
        return parse_leading_int(rest);
    }
    // Try `key=VALUE`
    let unquoted2 = format!("{}=", key);
    if let Some(pos) = text.find(&unquoted2) {
        let after = &text[pos + unquoted2.len()..];
        let rest = after.trim_start();
        return parse_leading_int(rest);
    }
    None
}

#[cfg(target_os = "macos")]
fn parse_leading_int(s: &str) -> Option<i64> {
    let end = s
        .find(|c: char| !c.is_ascii_digit() && c != '-')
        .unwrap_or(s.len());
    s[..end].parse().ok()
}

/// Extract a quoted string value: `"key" = "value"` → `value`.
#[cfg(target_os = "macos")]
fn extract_quoted_string(text: &str, key: &str) -> Option<String> {
    let pattern = format!("\"{}\"", key);
    let pos = text.find(&pattern)?;
    let after = text[pos + pattern.len()..].trim_start_matches(|c: char| c == ' ' || c == '=');
    if after.starts_with('"') {
        let inner = &after[1..];
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_sample_ioreg_output() {
        let sample = r#"
+-o AGXAcceleratorG14G  <class AGXAcceleratorG14G, id 0x100000217, registered, matched, active, busy 0, retain 28>
  {
    "gpu-core-count" = 10
    "model" = "Apple M2"
    "PerformanceStatistics" = {"Alloc system memory"=2594603008,"Device Utilization %"=16,"In use system memory"=505790464,"Renderer Utilization %"=15,"Tiler Utilization %"=16,"recoveryCount"=0,"lastRecoveryTime"=0}
  }
"#;
        let ts = chrono::Utc::now();
        let snap = parse_ioreg_accelerator(sample, ts);

        assert_eq!(snap.utilization_pct, Some(16.0));
        assert_eq!(snap.tiler_utilization_pct, Some(16.0));
        assert_eq!(snap.renderer_utilization_pct, Some(15.0));
        assert_eq!(snap.vram_used_bytes, Some(505_790_464));
        assert_eq!(snap.vram_alloc_bytes, Some(2_594_603_008));
        assert_eq!(snap.recovery_count, Some(0));
        assert_eq!(snap.model.as_deref(), Some("Apple M2"));
        assert_eq!(snap.core_count, Some(10));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn parse_empty_returns_nones() {
        let ts = chrono::Utc::now();
        let snap = parse_ioreg_accelerator("", ts);
        assert!(snap.utilization_pct.is_none());
        assert!(snap.model.is_none());
    }

    /// Live smoke test: calls real ioreg on macOS. Gated behind --ignored.
    #[cfg(target_os = "macos")]
    #[test]
    #[ignore]
    fn live_gpu_snapshot_has_utilization() {
        let snap = read_gpu_snapshot();
        // On any Mac with a GPU, utilization_pct should be Some
        assert!(
            snap.utilization_pct.is_some(),
            "Expected utilization_pct from real ioreg"
        );
        let pct = snap.utilization_pct.unwrap();
        assert!(pct >= 0.0 && pct <= 100.0, "utilization out of range: {}", pct);
        assert!(snap.model.is_some(), "Expected model string");
        if let Some(used) = snap.vram_used_bytes {
            assert!(used > 0, "Expected non-zero VRAM usage");
        }
    }
}
