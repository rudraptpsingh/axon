# mcp-station

Local hardware intelligence for AI coding agents. Zero cloud. Zero telemetry. Pure local.

mcp-station is an [MCP](https://modelcontextprotocol.io/) server that gives AI agents (Claude Desktop, Cursor, VS Code, Claude Code) real-time awareness of your Mac's hardware state: what process is slowing things down, how to fix it, and whether your machine can handle the next task.

```
$ mcp-station diagnose

[warn] Cursor (PID 1234)  --  210% CPU,  13.8GB RAM
       Impact: System is overloaded. Your session may freeze or crash.
       Fix:    Restart Cursor or close unused tabs (Cmd+W).
       Temp:   87C  [THROTTLING]
       Battery: Battery at 12% and discharging. Estimated 38 minutes remaining.
```

## Install

```bash
# From source (requires Rust toolchain)
cargo install --path crates/mcp-station-cli

# mcp-station auto-configures Claude Desktop, Cursor, and VS Code on first run.
# Just restart your agent after installing.
```

## What It Does

mcp-station exposes 4 MCP tools that any compatible agent can call:

| Tool | Purpose |
|---|---|
| `process_blame` | Identify the top culprit process, its impact severity, and a specific fix |
| `hw_snapshot` | CPU usage, die temperature, RAM used/total, pressure level, throttling state |
| `battery_status` | Battery percentage, charging state, estimated time remaining |
| `system_profile` | Machine model, chip, core count, total RAM, macOS version |

The hero tool is `process_blame`. When your AI session lags, the agent calls it and gets back:

```json
{
  "ok": true,
  "data": {
    "anomaly_type": "memory_pressure",
    "impact_level": "strained",
    "culprit": {"pid": 1234, "cmd": "Cursor", "cpu_pct": 210.0, "ram_gb": 13.8},
    "impact": "System is slowing down. Applications may lag or become unresponsive.",
    "fix": "Restart Cursor or close unused tabs (Cmd+W)."
  },
  "narrative": "Cursor (PID 1234, 210% CPU, 13.8GB RAM) -- System is slowing down..."
}
```

## How It Works

1. A background collector loop refreshes hardware state every 2 seconds via `sysinfo`
2. Per-process EWMA (Exponentially Weighted Moving Average) baselines detect anomalous resource usage
3. Multi-signal scoring (40% RAM + 30% CPU + 30% swap) classifies system health into 4 tiers
4. A persistence filter requires 3+ consecutive anomalous samples before escalating, avoiding false positives on transient spikes
5. Process-specific fix suggestions are returned for known resource hogs (Cursor, cargo, node, Docker, Ollama, etc.)

## CLI Commands

```bash
mcp-station serve              # Start MCP stdio server (default, used by agents)
mcp-station diagnose           # One-shot: collect 4s of data, print the culprit
mcp-station status             # Print current hardware snapshot as JSON
mcp-station setup <target>     # Configure an agent (claude-desktop, claude-code, cursor, vscode)
```

## Agent Setup

mcp-station auto-configures supported agents on first run. You can also set up manually:

```bash
mcp-station setup claude-desktop   # Writes claude_desktop_config.json
mcp-station setup claude-code      # Runs: claude mcp add mcp-station
mcp-station setup cursor           # Writes ~/.cursor/mcp.json
mcp-station setup vscode           # Writes VS Code user settings
```

Or add to any MCP-compatible agent's config manually:

```json
{
  "mcpServers": {
    "mcp-station": {
      "command": "/path/to/mcp-station",
      "args": ["serve"]
    }
  }
}
```

## Architecture

```
crates/
  mcp-station-core/     # Types, EWMA tracker, impact engine, collector loop
  mcp-station-server/   # MCP server (4 tools via rmcp)
  mcp-station-cli/      # Binary entry point
```

Key design decisions:
- **Privacy by architecture** -- no network calls, no telemetry, no cloud. Data never leaves your machine.
- **stdio transport** -- universal MCP compatibility with all current agents.
- **EWMA baselines** -- simple, effective anomaly detection at 2-second granularity.
- **In-memory only** -- no database, no persistence. Restart = fresh baseline.

## Requirements

- macOS (Apple Silicon or Intel)
- Rust 1.75+ (for building from source)

## License

MIT
