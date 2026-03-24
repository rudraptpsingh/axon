use sysinfo::Components;

/// Read the CPU die temperature via sysinfo Components (reads macOS SMC sensors).
/// Returns the maximum temperature found across CPU-related sensors.
/// Returns None if no CPU temperature sensors are accessible.
pub fn read_cpu_temp() -> Option<f64> {
    let components = Components::new_with_refreshed_list();

    let mut max_temp: Option<f32> = None;
    for component in &components {
        let label = component.label().to_lowercase();
        // Match Apple Silicon and Intel sensor label patterns
        if label.contains("cpu")
            || label.contains("die")
            || label.contains("core")
            || label.contains("package")
            || label.contains("soc")
            || label.contains("p-cores")
            || label.contains("e-cores")
        {
            // sysinfo 0.33: temperature() returns Option<f32>
            if let Some(temp) = component.temperature() {
                if temp > 0.0 && temp < 150.0 {
                    max_temp = Some(match max_temp {
                        Some(m) => m.max(temp),
                        None => temp,
                    });
                }
            }
        }
    }

    // If sysinfo found nothing, try Windows WMI thermal zone (requires admin on some machines)
    #[cfg(target_os = "windows")]
    if max_temp.is_none() {
        if let Some(t) = read_wmi_thermal_zone() {
            return Some(t);
        }
    }

    max_temp.map(|t| t as f64)
}

/// Windows fallback: Read MSAcpi_ThermalZoneTemperature via PowerShell.
/// Returns None if unavailable (non-admin, no ACPI thermal zone, etc.).
/// Temperature is in tenths of Kelvin in WMI, converted to Celsius.
#[cfg(target_os = "windows")]
fn read_wmi_thermal_zone() -> Option<f64> {
    use std::process::Command;
    let output = Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            "(Get-CimInstance MSAcpi_ThermalZoneTemperature -Namespace root/wmi -ErrorAction SilentlyContinue | Select-Object -First 1).CurrentTemperature",
        ])
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let raw: f64 = text.trim().parse().ok()?;
    // WMI returns temperature in tenths of Kelvin
    let celsius = (raw - 2732.0) / 10.0;
    if celsius > 0.0 && celsius < 150.0 {
        Some(celsius)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Diagnostic test: print what temperature sensors are visible.
    /// Gated behind --ignored so it doesn't run in CI.
    #[test]
    #[ignore]
    fn live_temperature_diagnostic() {
        let components = sysinfo::Components::new_with_refreshed_list();
        eprintln!("sysinfo found {} component(s):", components.len());
        for c in &components {
            eprintln!(
                "  label={:?} temp={:?} max={:?}",
                c.label(),
                c.temperature(),
                c.max()
            );
        }
        let result = read_cpu_temp();
        eprintln!("read_cpu_temp() = {:?}", result);
    }
}
