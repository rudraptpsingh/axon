Run the full end-to-end validation scenario.

Run: `bash scripts/e2e-real-scenario.sh`

Report which steps passed/failed:
1. Build release binary
2. --version / --help output
3. status command (JSON parse)
4. diagnose (4s collection)
5. setup cursor (config written)
6. setup invalid target (error handling)
7. MCP tool exercise (all 7 tools respond)

If any step fails, show the error output and suggest a fix.
