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

    max_temp.map(|t| t as f64)
}
