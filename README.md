# axon

**Your AI agent has no idea your machine is throttling.**

It keeps going. More tokens. More confusion. Same degraded hardware. A single `cargo build` pegs the CPU, thermal throttle kicks in, and your agent burns 3,000 tokens trying to figure out why everything is slow -- when the answer is one tool call away.

axon is an [MCP](https://modelcontextprotocol.io/) server that gives AI agents real-time hardware awareness. It tells the agent what process is slowing things down, how to fix it, and whether the machine can handle the next task.

Works with Claude Desktop, Cursor, VS Code, and Claude Code. macOS and Linux today. Windows coming.

![axon diagnose](tapes/demo.gif)

```
$ axon diagnose

[warn] Cursor (2 processes)  --  204% CPU,  0.2GB RAM
       Impact: System is under load. You may notice minor slowdowns.
       Fix:    Restart Cursor or close unused editor tabs (Cmd+W).
       Temp:   73C
       Battery: Battery at 80% and charging.
```

## Install

![Install and setup](tapes/demo-setup.gif)

```bash
# Homebrew (recommended)
brew tap rudraptpsingh/axon
brew install axon

# From source (requires Rust toolchain)
cargo install --path crates/axon-cli
```

After installing, configure your agents:

```bash
axon setup              # configures all detected agents
axon setup claude-code  # or configure a specific agent
```

Then restart your agent.

## What It Does

axon exposes 6 MCP tools that any compatible agent can call:

| Tool | Purpose |
|---|---|
| `process_blame` | Identify the top culprit process, its impact severity, and a specific fix. Detects multi-instance agent accumulation (Claude, Cursor, Windsurf, VS Code, Zed) |
| `hw_snapshot` | CPU usage, die temperature, RAM/disk used/total, pressure levels, throttling state, and a `headroom` field (adequate/limited/insufficient) for pre-task gating |
| `battery_status` | Battery percentage, charging state, estimated time remaining |
| `system_profile` | Machine model, chip, core count, total RAM, OS version |
| `hardware_trend` | Historical CPU/RAM/temp trends over a time window (last_1h to last_30d) |
| `session_health` | Retrospective session summary: snapshot count, alert count, peak/avg CPU/RAM/temp, worst impact level since a given timestamp |

The hero tool is `process_blame`. When your AI session lags, the agent calls it and gets back:

![Detect, blame, fix, verify](tapes/demo-detect.gif)

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

Without hardware context, the agent guesses: "Let me try -j 4 instead of -j 8... still slow... let me check ps aux..." That's 2,000-3,000 tokens of confused exploration. With `process_blame`, it gets the answer in ~200 tokens and can act immediately.

## For AI Agents

If you are an AI agent with axon available as an MCP tool, here is when to use each tool:

- **Session start** -- call `system_profile` once to understand the machine you are running on (chip, cores, RAM, OS). This is cached and cheap.
- **Before starting a heavy task** (large build, test suite, code generation) -- call `hw_snapshot` and check the `headroom` field. If `headroom` is `insufficient`, warn the user or defer the task. If `limited`, proceed with caution.
- **Build slow or session lagging** -- call `process_blame` to identify what is causing the slowdown and get a specific fix. It also detects multi-instance agent accumulation (e.g., 4 Claude sessions eating 60GB RAM).
- **Battery concerns** -- call `battery_status` to check remaining charge. If battery is low and not charging, warn the user before starting long-running tasks.
- **Investigating performance over time** -- call `hardware_trend` with a time range (last_1h, last_6h, last_24h, last_7d, last_30d) to see if the machine is degrading or if there is a pattern.
- **End of long session** -- call `session_health` for a retrospective summary: how many alerts fired, what was the worst state, peak CPU/RAM/temp since session start.

All tools return structured JSON with an `ok` boolean, timestamp, data payload, and a human-readable `narrative` field.

## How It Works

1. A background collector loop refreshes hardware state every 2 seconds via `sysinfo`
2. Per-process EWMA (Exponentially Weighted Moving Average) baselines detect anomalous resource usage
3. Process grouping aggregates child processes by app (e.g., 47 Chrome helpers become one "Google Chrome" group)
4. Multi-signal scoring (40% RAM + 30% CPU + 30% swap) classifies system health into 4 tiers
5. A persistence filter requires 2+ consecutive anomalous samples before escalating, avoiding false positives on transient spikes
6. Process-specific fix suggestions are returned for known resource hogs (Cursor, cargo, node, Docker, Ollama, etc.)
7. Hardware snapshots and alerts are persisted to a local SQLite database for trend queries and alert history

## CLI Commands

```bash
axon serve              # Start MCP stdio server (default, used by agents)
axon diagnose           # One-shot: collect 4s of data, print the culprit
axon status             # Print current hardware snapshot as JSON
axon query <tool>       # Call an MCP tool directly (e.g., axon query process_blame)
axon setup <target>     # Configure an agent (claude-desktop, claude-code, cursor, vscode)
```

## Agent Setup

Run `axon setup` after installing to configure your agents:

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

## Alerts

axon fires edge-triggered alerts on state transitions -- not every tick. RAM pressure spikes, disk pressure warnings, thermal throttle onset, impact escalation. Delivered via webhook POST or MCP logging notifications.

![Live alert firing](tapes/demo-alerts-live.gif)

Configure with a one-line flag or a config file:

![Alert config](tapes/demo-alerts-config.gif)

```bash
# One-line flag
axon serve --alert-webhook myapp=https://yourapp.com/alerts

# Or config file at ~/.config/axon/alert-dispatch.json
```

After an alert fires, the agent queries full context:

![Agent queries blame after alert](tapes/demo-alerts-query.gif)

## Architecture

```
crates/
  axon-core/     # Types, EWMA tracker, impact engine, collector loop, alert dispatch, SQLite persistence
  axon-server/   # MCP server (6 tools via rmcp)
  axon-cli/      # Binary entry point
```

Key design decisions:
- **Privacy by architecture** -- no network calls, no telemetry, no cloud. Data never leaves your machine.
- **stdio transport** -- universal MCP compatibility with all current agents.
- **EWMA baselines** -- simple, effective anomaly detection at 2-second granularity.
- **SQLite persistence** -- snapshots every 10s, alerts on state transitions. Powers `hardware_trend` and alert history.
- **Edge-triggered alerts** -- fire once on state transitions (Normal->Warn, Healthy->Strained), not on every tick. Covers RAM pressure, disk pressure, thermal throttling, and impact escalation. Delivered via webhook POST or MCP logging notifications.

## Requirements

- macOS (Apple Silicon or Intel) and Linux. Windows support planned -- the underlying `sysinfo` crate already supports it.
- Rust 1.75+ (for building from source)

## License

MIT
