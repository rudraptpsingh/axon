# axon

**Your AI agent has no idea your machine is throttling.**

Your M4 MacBook Air is thermal throttling. Claude keeps spawning builds. A single `cargo build` pegs the CPU, thermal throttle kicks in, and your agent burns 3,000 tokens trying to figure out why everything is slow -- when the answer is one tool call away.

This is not theoretical. There are [15 open GitHub issues](docs/problem-validation.md) in the claude-code repository documenting OOM crashes, kernel panics, and 60GB RAM accumulation from idle sessions. A [METR study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/) found developers using AI tools were 19% slower -- but thought they were 20% faster. The bottleneck moved to the hardware, and nobody told the agent.

axon is an [MCP](https://modelcontextprotocol.io/) server that gives AI agents real-time hardware awareness. It tells the agent what process is slowing things down, how to fix it, and whether the machine can handle the next task. One tool call. ~200 tokens. Structured answer.

Works with Claude Desktop, Cursor, VS Code, and Claude Code. macOS and Linux today. Windows coming.

## Privacy

Zero network calls. Your process names, load patterns, and hardware state never leave your machine. This is not a config option -- it is enforced as a hard design constraint. No telemetry, no analytics, no cloud. Ever.

---

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
brew install rudraptpsingh/tap/axon

# From source (requires Rust toolchain)
cargo install --path crates/axon-cli
```

After installing, configure your agents:

```bash
axon setup              # configures all detected agents
axon setup claude-code  # or configure a specific agent
```

Then restart your agent.

## What happens when your agent has axon

**Your build is slow.** The agent calls `process_blame` and gets back: Cursor is eating 204% CPU across 2 processes. Impact: degrading. Fix: restart Cursor or close unused tabs. 200 tokens. Done. Without axon, the agent runs `ps aux`, `top -l 1`, `vm_stat`, parses the output, guesses wrong, and burns 2,000-3,000 tokens.

**You are about to run a test suite.** The agent calls `hw_snapshot` and checks the `headroom` field. It comes back `insufficient` -- RAM is at 73%, CPU is saturated. The agent warns you: "System is under pressure, tests may be unreliable. Want to wait?" Without axon, the agent runs the tests, they flake, and it blames a race condition.

**Your session has been running 6 hours.** The agent calls `session_health` and sees: 3 alerts fired, peak RAM 14GB, worst impact level was Strained. It tells you: "This session has been rough on your machine. Consider restarting before the next heavy task." Without axon, it has no idea.

**In controlled testing**, an agent using axon's headroom signal reduced P95 latency by 50.3% under stress by switching from async to sync processing. [Full results here.](agent_behavior_report.md)

### Tool reference

| Tool | What the agent gets |
|---|---|
| `process_blame` | Top culprit process, anomaly type, impact severity, specific fix. Detects multi-instance agent accumulation (Claude, Cursor, Windsurf, VS Code, Zed) |
| `hw_snapshot` | CPU %, die temp, RAM/disk pressure levels, throttling state, `headroom` field (adequate/limited/insufficient) for pre-task gating |
| `battery_status` | Battery percentage, charging state, time remaining |
| `system_profile` | Machine model, chip, core count, total RAM, OS version |
| `hardware_trend` | CPU/RAM/temp trends over time (last_1h to last_30d) |
| `session_health` | Alert count, worst impact level, peak CPU/RAM/temp since a given timestamp |

All tools return structured JSON with an `ok` boolean, timestamp, data payload, and a human-readable `narrative` field.

![Detect, blame, fix, verify](tapes/demo-detect.gif)

## For AI Agents

If you are an AI agent with axon available:

- **Session start** -- call `system_profile` to know the machine (chip, cores, RAM)
- **Before heavy tasks** -- call `hw_snapshot`, check `headroom`. Defer if `insufficient`, proceed with caution if `limited`
- **Slow build or lag** -- call `process_blame` for the culprit and a specific fix
- **Battery concerns** -- call `battery_status` before long-running work
- **Performance patterns** -- call `hardware_trend` to correlate failures with resource anomalies
- **End of long session** -- call `session_health` for a retrospective summary

## How It Works

1. A background collector refreshes hardware state every 2 seconds via `sysinfo`
2. Per-process EWMA baselines detect anomalous resource usage -- transient spikes are filtered out
3. Process grouping aggregates child processes by app (47 Chrome helpers become one "Google Chrome" group)
4. Multi-signal scoring (RAM + CPU + swap) classifies system health into 4 tiers: Healthy, Degrading, Strained, Critical

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
- **Privacy by architecture** -- no network calls, no telemetry, no cloud
- **stdio transport** -- universal MCP compatibility with all current agents
- **SQLite persistence** -- snapshots every 10s, alerts on state transitions, powers trend queries
- **Edge-triggered alerts** -- fire once on state transitions (Normal->Warn, Healthy->Strained), not on every tick

## Requirements

- macOS (Apple Silicon or Intel) and Linux. Windows support planned -- the underlying `sysinfo` crate already supports it.
- Rust 1.75+ (for building from source)

## See also

- [The evidence: 15 GitHub issues and research](docs/problem-validation.md) -- why this problem exists
- [Agent adaptation test](agent_behavior_report.md) -- 50.3% latency reduction under stress
- [Parallel agent comparison](comparative_stress_test_results/comparison_report.md) -- blind vs informed agents on one machine

## License

MIT
