Check what is eating resources right now.

Run: `axon query process_blame`

Report:
- Top culprit process (name, PID, CPU%, RAM)
- Process group (if grouped, e.g. Chrome helpers)
- Anomaly type (cpu_saturation / agent_accumulation / none)
- Impact level and score
- Fix suggestion
- Whether the blame attribution looks correct
