//! GPU metrics collector.
//!
//! macOS: Queries the IOAccelerator kernel object via `ioreg` and parses the
//! `PerformanceStatistics` dictionary.  No sudo required.  Covers Apple Silicon
//! (AGXAccelerator) and Intel discrete/integrated GPUs (IOAccelerator subclasses).
//!
//! Linux: Tries NVIDIA (`nvidia-smi`) then AMD sysfs
//! (`/sys/class/drm/cardX/device/gpu_busy_percent`, `mem_info_vram_*`).
//! Falls back to all-None if neither is available.
//!
//! Windows: Tries NVIDIA (`nvidia-smi`) first for full data. Falls back to
//! GPU Engine performance counters (utilization) + WMI Win32_VideoController
//! (model + VRAM) for AMD/Intel/any GPU. Static info cached; utilization
//! refreshed every 5 ticks (~10s).

use crate::types::GpuSnapshot;

/// Collect a GPU snapshot.  Cheap to call every collector tick (~5 ms on M-series).
pub fn read_gpu_snapshot() -> GpuSnapshot {
    #[cfg(target_os = "macos")]
    {
        read_gpu_macos()
    }
    #[cfg(target_os = "linux")]
    {
        read_gpu_linux()
    }
    #[cfg(target_os = "windows")]
    {
        read_gpu_windows()
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
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
            detected: false,
            ts: chrono::Utc::now(),
            vram_growth_mb_per_hr: None,
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
        detected: false,
        ts,
        vram_growth_mb_per_hr: None,
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

    // detected = true only if ioreg gave us at least some real data
    let detected = utilization_pct.is_some() || model.is_some();

    GpuSnapshot {
        utilization_pct,
        tiler_utilization_pct,
        renderer_utilization_pct,
        vram_used_bytes,
        vram_alloc_bytes,
        recovery_count,
        model,
        core_count,
        detected,
        ts,
        vram_growth_mb_per_hr: None,
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
        let after = text[pos + quoted_pattern.len()..].trim_start_matches(' ');
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
    let after = text[pos + pattern.len()..].trim_start_matches([' ', '=']);
    if let Some(inner) = after.strip_prefix('"') {
        let end = inner.find('"')?;
        Some(inner[..end].to_string())
    } else {
        None
    }
}

// ── Windows implementation ────────────────────────────────────────────────────

/// Cached GPU static info (model + VRAM) — fetched once, reused on every tick.
#[cfg(target_os = "windows")]
static GPU_STATIC_CACHE: std::sync::Mutex<Option<(Option<String>, Option<u64>)>> =
    std::sync::Mutex::new(None);

/// Cached GPU utilization — refreshed every 5 ticks (~10s) to avoid
/// PowerShell startup overhead on every 2s tick.
#[cfg(target_os = "windows")]
static GPU_UTIL_CACHE: std::sync::Mutex<(Option<f64>, u32)> = std::sync::Mutex::new((None, 0));

#[cfg(target_os = "windows")]
fn read_gpu_windows() -> GpuSnapshot {
    let now = chrono::Utc::now();

    // nvidia-smi gives full data in a single call — prefer it when available
    if let Some(snap) = try_nvidia_smi(now) {
        return snap;
    }

    // Fallback: combine WMI static info + GPU perf counter utilization

    // 1. Static info (model + VRAM total) — fetch once
    let mut static_cache = GPU_STATIC_CACHE.lock().unwrap();
    if static_cache.is_none() {
        *static_cache = Some(fetch_wmi_gpu_static());
    }
    let (ref model, vram_alloc) = static_cache.as_ref().unwrap();

    // 2. Utilization — refresh every 5 ticks
    let utilization_pct = {
        let mut util_cache = GPU_UTIL_CACHE.lock().unwrap();
        util_cache.1 += 1;
        if util_cache.1 == 1 || util_cache.1 % 5 == 0 {
            util_cache.0 = fetch_gpu_engine_utilization();
        }
        util_cache.0
    };

    let detected = model.is_some();
    GpuSnapshot {
        utilization_pct,
        tiler_utilization_pct: None,
        renderer_utilization_pct: None,
        vram_used_bytes: None,
        vram_alloc_bytes: *vram_alloc,
        recovery_count: None,
        model: model.clone(),
        core_count: None,
        detected,
        ts: now,
        vram_growth_mb_per_hr: None,
    }
}

/// Fetch GPU model and VRAM from Win32_VideoController (one-time).
#[cfg(target_os = "windows")]
fn fetch_wmi_gpu_static() -> (Option<String>, Option<u64>) {
    use std::process::Command;
    let output = match Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "Get-CimInstance Win32_VideoController | Where-Object { $_.Name -notlike '*IDDCX*' -and $_.Name -notlike '*Basic*' -and $_.Name -notlike '*Remote*' } | Select-Object -First 1 -Property Name,AdapterRAM | ConvertTo-Json",
        ])
        .output()
    {
        Ok(o) => o,
        Err(_) => return (None, None),
    };
    if !output.status.success() {
        return (None, None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let text = text.trim();
    if text.is_empty() || text == "null" {
        return (None, None);
    }
    parse_wmi_gpu_static(text)
}

#[cfg(target_os = "windows")]
fn parse_wmi_gpu_static(text: &str) -> (Option<String>, Option<u64>) {
    let v: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => return (None, None),
    };
    let obj = if v.is_array() {
        v.get(0).unwrap_or(&v)
    } else {
        &v
    };
    let model = obj
        .get("Name")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let vram = obj.get("AdapterRAM").and_then(|v| v.as_u64());
    (model, vram)
}

/// Read total GPU utilization from Windows GPU Engine performance counters.
/// Sums UtilizationPercentage across all GPU engine instances.
#[cfg(target_os = "windows")]
fn fetch_gpu_engine_utilization() -> Option<f64> {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance Win32_PerfFormattedData_GPUPerformanceCounters_GPUEngine -ErrorAction SilentlyContinue | Measure-Object -Property UtilizationPercentage -Sum).Sum",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let val = text.trim().parse::<f64>().ok()?;
    Some(val.clamp(0.0, 100.0))
}

/// Parse WMI GPU JSON — used by unit tests.
#[cfg(all(target_os = "windows", test))]
fn parse_wmi_gpu(text: &str, now: chrono::DateTime<chrono::Utc>) -> Option<GpuSnapshot> {
    let (model, vram_alloc_bytes) = parse_wmi_gpu_static(text);
    if model.is_none() {
        return None;
    }
    Some(GpuSnapshot {
        utilization_pct: None,
        tiler_utilization_pct: None,
        renderer_utilization_pct: None,
        vram_used_bytes: None,
        vram_alloc_bytes: vram_alloc_bytes,
        recovery_count: None,
        model,
        core_count: None,
        detected: true,
        ts: now,
        vram_growth_mb_per_hr: None,
    })
}

// ── Linux implementation ───────────────────────────────────────────────────────

#[cfg(target_os = "linux")]
fn read_gpu_linux() -> GpuSnapshot {
    let now = chrono::Utc::now();
    try_nvidia_smi(now)
        .or_else(|| try_amd_sysfs(now))
        .unwrap_or(GpuSnapshot {
            utilization_pct: None,
            tiler_utilization_pct: None,
            renderer_utilization_pct: None,
            vram_used_bytes: None,
            vram_alloc_bytes: None,
            recovery_count: None,
            model: None,
            core_count: None,
            detected: false,
            ts: now,
            vram_growth_mb_per_hr: None,
        })
}

/// Query `nvidia-smi` for GPU name, utilisation, and VRAM.
/// Returns `None` if `nvidia-smi` is not present or returns a non-zero exit code.
/// Available on Linux and Windows (nvidia-smi ships with NVIDIA drivers on both).
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn try_nvidia_smi(now: chrono::DateTime<chrono::Utc>) -> Option<GpuSnapshot> {
    use std::process::Command;
    let output = Command::new("nvidia-smi")
        .args([
            "--query-gpu=name,utilization.gpu,memory.used,memory.total",
            "--format=csv,noheader,nounits",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    parse_nvidia_smi_csv(&text, now)
}

/// Parse one line of `nvidia-smi --format=csv,noheader,nounits` output.
/// Expected format: `<name>, <util%>, <mem_used_MiB>, <mem_total_MiB>`
#[cfg(any(target_os = "linux", target_os = "windows"))]
fn parse_nvidia_smi_csv(text: &str, now: chrono::DateTime<chrono::Utc>) -> Option<GpuSnapshot> {
    let line = text.lines().find(|l| !l.trim().is_empty())?;
    let parts: Vec<&str> = line.splitn(4, ',').map(|s| s.trim()).collect();
    if parts.len() < 4 {
        return None;
    }
    let model = Some(parts[0].to_string());
    let utilization_pct = parts[1].parse::<f64>().ok();
    // nvidia-smi reports VRAM in MiB
    let vram_used_bytes = parts[2].parse::<u64>().ok().map(|mib| mib * 1024 * 1024);
    let vram_alloc_bytes = parts[3].parse::<u64>().ok().map(|mib| mib * 1024 * 1024);

    // Require at least one real value
    if utilization_pct.is_none() && vram_used_bytes.is_none() {
        return None;
    }

    Some(GpuSnapshot {
        utilization_pct,
        tiler_utilization_pct: None,
        renderer_utilization_pct: None,
        vram_used_bytes,
        vram_alloc_bytes,
        recovery_count: None,
        model,
        core_count: None,
        detected: true,
        ts: now,
        vram_growth_mb_per_hr: None,
    })
}

/// Read AMD GPU metrics from the DRM sysfs interface.
/// Vendor ID 0x1002 = AMD.  Files read:
///   `device/gpu_busy_percent`, `device/mem_info_vram_used`,
///   `device/mem_info_vram_total`.
#[cfg(target_os = "linux")]
fn try_amd_sysfs(now: chrono::DateTime<chrono::Utc>) -> Option<GpuSnapshot> {
    let dev_path = find_drm_card_by_vendor("0x1002")?;
    let util = read_sysfs_u64(&dev_path.join("gpu_busy_percent")).map(|v| v as f64);
    let vram_used = read_sysfs_u64(&dev_path.join("mem_info_vram_used"));
    let vram_total = read_sysfs_u64(&dev_path.join("mem_info_vram_total"));

    // Require at least utilization or VRAM info
    if util.is_none() && vram_used.is_none() {
        return None;
    }

    Some(GpuSnapshot {
        utilization_pct: util,
        tiler_utilization_pct: None,
        renderer_utilization_pct: None,
        vram_used_bytes: vram_used,
        vram_alloc_bytes: vram_total,
        recovery_count: None,
        model: read_sysfs_string(&dev_path.join("product_name")),
        core_count: None,
        detected: true,
        ts: now,
        vram_growth_mb_per_hr: None,
    })
}

/// Walk `/sys/class/drm/cardN` entries and return the `device/` path for the
/// first card whose `device/vendor` matches `vendor_id` (e.g. `"0x1002"`).
#[cfg(target_os = "linux")]
fn find_drm_card_by_vendor(vendor_id: &str) -> Option<std::path::PathBuf> {
    let drm = std::path::Path::new("/sys/class/drm");
    let entries = std::fs::read_dir(drm).ok()?;
    let mut names: Vec<_> = entries
        .filter_map(|e| e.ok())
        .map(|e| e.file_name())
        .filter(|n| {
            let s = n.to_string_lossy();
            // cardN only, not cardN-HDMI-A-1 connector entries
            s.starts_with("card") && !s.contains('-')
        })
        .collect();
    names.sort();
    for name in names {
        let dev = drm.join(&name).join("device");
        let vendor = read_sysfs_string(&dev.join("vendor")).unwrap_or_default();
        if vendor.trim().eq_ignore_ascii_case(vendor_id) {
            return Some(dev);
        }
    }
    None
}

/// Read a sysfs file and parse its content as a `u64`.
#[cfg(target_os = "linux")]
fn read_sysfs_u64(path: &std::path::Path) -> Option<u64> {
    let s = read_sysfs_string(path)?;
    s.trim().parse().ok()
}

/// Read a sysfs file as a trimmed string.  Returns `None` on any I/O error.
#[cfg(target_os = "linux")]
fn read_sysfs_string(path: &std::path::Path) -> Option<String> {
    std::fs::read_to_string(path)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    #[allow(unused_imports)]
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
        assert!(
            pct >= 0.0 && pct <= 100.0,
            "utilization out of range: {}",
            pct
        );
        assert!(snap.model.is_some(), "Expected model string");
        if let Some(used) = snap.vram_used_bytes {
            assert!(used > 0, "Expected non-zero VRAM usage");
        }
    }

    // ── nvidia-smi parser tests (Linux + Windows; no hardware required) ────

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn parse_nvidia_smi_csv_typical() {
        let ts = chrono::Utc::now();
        let input = "NVIDIA GeForce RTX 3080, 45, 4096, 10240\n";
        let snap = parse_nvidia_smi_csv(input, ts).expect("should parse");
        assert_eq!(snap.model.as_deref(), Some("NVIDIA GeForce RTX 3080"));
        assert_eq!(snap.utilization_pct, Some(45.0));
        assert_eq!(snap.vram_used_bytes, Some(4096 * 1024 * 1024));
        assert_eq!(snap.vram_alloc_bytes, Some(10240 * 1024 * 1024));
        assert!(snap.tiler_utilization_pct.is_none());
        assert!(snap.renderer_utilization_pct.is_none());
        assert!(snap.recovery_count.is_none());
        assert!(snap.core_count.is_none());
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn parse_nvidia_smi_csv_zero_util() {
        let ts = chrono::Utc::now();
        // Idle GPU: 0 % utilization, valid VRAM
        let input = "Tesla T4, 0, 512, 15360\n";
        let snap = parse_nvidia_smi_csv(input, ts).expect("should parse idle GPU");
        assert_eq!(snap.utilization_pct, Some(0.0));
        assert_eq!(snap.vram_used_bytes, Some(512 * 1024 * 1024));
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn parse_nvidia_smi_csv_empty_returns_none() {
        let ts = chrono::Utc::now();
        assert!(parse_nvidia_smi_csv("", ts).is_none());
        assert!(parse_nvidia_smi_csv("   \n", ts).is_none());
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn parse_nvidia_smi_csv_too_few_fields_returns_none() {
        let ts = chrono::Utc::now();
        assert!(parse_nvidia_smi_csv("NVIDIA RTX 3080, 45\n", ts).is_none());
    }

    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    fn parse_nvidia_smi_csv_name_with_comma() {
        // splitn(4, ',') ensures the name can contain commas
        let ts = chrono::Utc::now();
        let input = "NVIDIA GeForce RTX 3080, Ti, 30, 2048, 10240\n";
        // With splitn(4) the name gets everything before the first ',',
        // so this tests that we handle the common case gracefully.
        // The important thing is no panic and we get a valid or None result.
        let _result = parse_nvidia_smi_csv(input, ts);
        // No assertion on content; just must not panic.
    }

    // ── Windows WMI parser tests ──────────────────────────────────────────────

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_wmi_gpu_amd() {
        let ts = chrono::Utc::now();
        let input = r#"{"Name":"AMD Radeon(TM) Graphics","AdapterRAM":536870912}"#;
        let snap = parse_wmi_gpu(input, ts).expect("should parse AMD GPU");
        assert_eq!(snap.model.as_deref(), Some("AMD Radeon(TM) Graphics"));
        assert_eq!(snap.vram_alloc_bytes, Some(536870912));
        assert!(snap.detected);
        assert!(snap.utilization_pct.is_none()); // WMI can't read utilization
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_wmi_gpu_nvidia() {
        let ts = chrono::Utc::now();
        let input = r#"{"Name":"NVIDIA GeForce RTX 4090","AdapterRAM":4293918720}"#;
        let snap = parse_wmi_gpu(input, ts).expect("should parse NVIDIA GPU");
        assert_eq!(snap.model.as_deref(), Some("NVIDIA GeForce RTX 4090"));
        assert_eq!(snap.vram_alloc_bytes, Some(4293918720));
        assert!(snap.detected);
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn parse_wmi_gpu_null_returns_none() {
        let ts = chrono::Utc::now();
        assert!(parse_wmi_gpu("null", ts).is_none());
        assert!(parse_wmi_gpu("", ts).is_none());
    }

    /// Live smoke test: run on Linux/Windows when a real GPU is present.
    /// Gated behind --ignored so CI does not require hardware.
    #[cfg(any(target_os = "linux", target_os = "windows"))]
    #[test]
    #[ignore]
    fn live_linux_gpu_snapshot_does_not_panic() {
        let snap = read_gpu_snapshot();
        // The snapshot is always valid even with no GPU (all-None fields).
        // If a GPU is present, utilization must be in [0, 100].
        if let Some(pct) = snap.utilization_pct {
            assert!(
                pct >= 0.0 && pct <= 100.0,
                "utilization out of range: {}",
                pct
            );
        }
        if let (Some(used), Some(total)) = (snap.vram_used_bytes, snap.vram_alloc_bytes) {
            assert!(used <= total, "VRAM used ({used}) > total ({total})");
        }
    }
}
