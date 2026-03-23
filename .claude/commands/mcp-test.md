Exercise all 7 MCP tools via the protocol.

Run: `python3 scripts/mcp_exercise_all_tools.py ./target/release/axon`

Report:
- Were all 7 tools found in tools/list? (hw_snapshot, process_blame, battery_status, system_profile, hardware_trend, session_health, gpu_snapshot)
- Did each tool return valid JSON with ok=true?
- Any errors, crashes, or unexpected responses?
- Did the axon serve process stay alive throughout?
