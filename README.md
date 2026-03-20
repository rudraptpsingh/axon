# axon

Local hardware intelligence for AI coding agents. Zero cloud. Zero telemetry. Pure local.

axon is an [MCP](https://modelcontextprotocol.io/) server that gives AI agents (Claude Desktop, Cursor, VS Code, Claude Code) real-time awareness of your Mac's hardware state: what process is slowing things down, how to fix it, and whether your machine can handle the next task.

```
$ axon diagnose

[warn] Cursor (2 processes)  --  204% CPU,  0.2GB RAM
       Impact: System is under load. You may notice minor slowdowns.
       Fix:    Restart Cursor or close unused editor tabs (Cmd+W).
       Temp:   73C
       Battery: Battery at 80% and charging.
```

## Install

```bash
# Homebrew (recommended)
brew tap rudraptpsingh/axon
brew install axon

# From source (requires Rust toolchain)
cargo install --path crates/axon-cli
```

axon auto-configures Claude Desktop, Cursor, and VS Code on first run. Just restart your agent after installing.

## What It Does

axon exposes 4 MCP tools that any compatible agent can call:

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
    "anomaly_type": "general_slowdown",
    "impact_level": "degrading",
    "culprit": {"pid": 80630, "cmd": "Cursor Helper (Renderer)", "cpu_pct": 101.1, "ram_gb": 0.1},
    "culprit_group": {
      "name": "Cursor",
      "process_count": 2,
      "total_cpu_pct": 201.5,
      "total_ram_gb": 0.1
    },
    "impact": "System is under load. You may notice minor slowdowns.",
    "fix": "Restart Cursor or close unused editor tabs (Cmd+W)."
  },
  "narrative": "Cursor (0.1GB across 2 processes, 202% CPU) -- System is under load..."
}
```

## How It Works

1. A background collector loop refreshes hardware state every 2 seconds via `sysinfo`
2. Per-process EWMA (Exponentially Weighted Moving Average) baselines detect anomalous resource usage
3. Process grouping aggregates child processes by app (e.g., 47 Chrome helpers become one "Google Chrome" group)
4. Multi-signal scoring (40% RAM + 30% CPU + 30% swap) classifies system health into 4 tiers
5. A persistence filter requires 3+ consecutive anomalous samples before escalating, avoiding false positives on transient spikes
6. Process-specific fix suggestions are returned for known resource hogs (Cursor, cargo, node, Docker, Ollama, etc.)

## CLI Commands

```bash
axon serve              # Start MCP stdio server (default, used by agents)
axon diagnose           # One-shot: collect 4s of data, print the culprit
axon status             # Print current hardware snapshot as JSON
axon setup <target>     # Configure an agent (claude-desktop, claude-code, cursor, vscode)
```

## Agent Setup

axon auto-configures supported agents on first run. You can also set up manually:

```bash
axon setup claude-desktop   # Writes claude_desktop_config.json
axon setup claude-code      # Runs: claude mcp add axon
axon setup cursor           # Writes ~/.cursor/mcp.json
axon setup vscode           # Writes VS Code user settings
```

Or add to any MCP-compatible agent's config manually:

```json
{
  "mcpServers": {
    "axon": {
      "command": "/path/to/axon",
      "args": ["serve"]
    }
  }
}
```

## Architecture

```
crates/
  axon-core/     # Types, EWMA tracker, impact engine, collector loop
  axon-server/   # MCP server (4 tools via rmcp)
  axon-cli/      # Binary entry point
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
