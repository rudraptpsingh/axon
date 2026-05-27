# axon

**Your AI agent has no idea when the machine under it is the bottleneck.**

Your laptop is thermal throttling. Cursor has old helper processes hanging around. Codex has multiple MCP servers still alive from earlier sessions. A single build pegs the CPU, the agent keeps spawning tool workers, and it burns tokens trying to debug "slow code" when the real problem is local runtime pressure.

This is not theoretical. There are [15 open GitHub issues](docs/problem-validation.md) documenting OOM crashes, kernel panics, runaway session files, stuck tool calls, and RAM accumulation from idle agent sessions. A [METR study](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/) found developers using AI tools were 19% slower -- but thought they were 20% faster. The bottleneck moved to the hardware, and nobody told the agent.

axon is an [MCP](https://modelcontextprotocol.io/) server that gives AI agents real-time local runtime awareness. It tells the agent what process is slowing things down, whether the host can handle the next task, how much parallelism is safe, and whether accumulated agent runtimes are creating business-impacting waste. One tool call. Structured JSON. No cloud.

Works with Claude Desktop, Claude Code, Cursor, and VS Code. macOS, Linux, and Windows.

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
axon setup cursor
axon setup vscode
```

Then restart your agent.

## What happens when your agent has axon

**Your build is slow.** The agent calls `process_blame` and gets back: Cursor is eating 204% CPU across 2 processes. Impact: degrading. Fix: restart Cursor or close unused tabs. 200 tokens. Done. Without axon, the agent runs `ps aux`, `top -l 1`, `vm_stat`, parses the output, guesses wrong, and burns 2,000-3,000 tokens.

**You are about to run a test suite.** The agent calls `workload_advice` with `{"kind":"test","requested_parallelism":4}`. Axon returns `defer`, `safe_parallelism: 1`, and concrete reasons: too many MCP servers, stale agent sessions, RAM pressure, or UI process CPU burn. The agent still performs useful lightweight work, but avoids spawning a parallel test storm on a pressured host.

**Your agent environment has accumulated.** The agent calls `agent_runtime_health` and sees Codex, Claude, Cursor, MCP servers, renderers, tool workers, stale processes, duplicate MCP groups, and workflow impact. It can say: "Do not spawn another browser MCP. Reuse the existing one or clean up stale sessions first." This is the difference between one productive agent and a workstation slowly filling with invisible helpers.

**Your session has been running 6 hours.** The agent calls `session_health` and sees: 3 alerts fired, peak RAM 14GB, worst impact level was Strained. It tells you: "This session has been rough on your machine. Consider restarting before the next heavy task." Without axon, it has no idea.

**In app-development A/B testing**, the same app task was run with and without Axon policy. Both versions produced identical app output. Axon avoided 4 extra MCP/tool helpers, capped risky parallelism from 4 to 1, preserved useful validation time, and app runtime did not degrade in the measured run. See [app_dev_ab_report.md](app_dev_ab_report.md).

**For business demos**, Axon can produce a scorecard with risky tool spawns avoided, parallel workers avoided, estimated credits saved, estimated developer time protected, and stale local runtime count. See [docs/value-scorecard.md](docs/value-scorecard.md) and [docs/agent-platform-demo.md](docs/agent-platform-demo.md).

### Tool reference

| Tool | What the agent gets |
|---|---|
| `process_blame` | Top culprit process, anomaly type, impact severity, specific fix. Detects multi-instance agent accumulation (Claude, Cursor, Windsurf, VS Code, Zed) |
| `hw_snapshot` | CPU %, die temp, RAM/disk pressure levels, throttling state, `headroom` field (adequate/limited/insufficient) for pre-task gating |
| `workload_advice` | A run/defer/degrade recommendation for builds, tests, Docker, browser automation, GPU jobs, subagents, or MCP-heavy workflows. Includes safe parallelism and risk reasons |
| `agent_runtime_health` | Cross-agent local runtime inventory: Codex/Claude/Cursor/Windsurf/Zed processes, MCP server count, stale sessions, duplicate MCP groups, UI/renderer pressure, and business workflow impact |
| `battery_status` | Battery percentage, charging state, time remaining |
| `system_profile` | Machine model, chip, core count, total RAM, OS version |
| `hardware_trend` | CPU/RAM/temp trends over time (last_1h to last_30d) |
| `session_health` | Alert count, worst impact level, peak CPU/RAM/temp since a given timestamp |
| `gpu_snapshot` | Local GPU detection, utilization, VRAM usage/allocation, and a narrative for ML or graphics-heavy work |

All tools return structured JSON with an `ok` boolean, timestamp, data payload, and a human-readable `narrative` field.

![Detect, blame, fix, verify](tapes/demo-detect.gif)

## For AI Agents

If you are an AI agent with axon available:

- **Session start** -- call `system_profile` to know the machine (chip, cores, RAM)
- **Before heavy tasks** -- call `hw_snapshot`, check `headroom`. Defer if `insufficient`, proceed with caution if `limited`
- **Before builds/tests/Docker/browser automation/subagents** -- call `workload_advice` with the workload kind and requested parallelism. Use `safe_parallelism` instead of guessing
- **When tool workers or MCP servers accumulate** -- call `agent_runtime_health` before spawning more local servers, browser controllers, or subagents
- **Slow build or lag** -- call `process_blame` for the culprit and a specific fix
- **Battery concerns** -- call `battery_status` before long-running work
- **Performance patterns** -- call `hardware_trend` to correlate failures with resource anomalies
- **End of long session** -- call `session_health` for a retrospective summary

## How It Works

1. A background collector refreshes hardware state every 2 seconds via `sysinfo`
2. Per-process EWMA baselines detect anomalous resource usage -- transient spikes are filtered out
3. Process grouping aggregates child processes by app (47 Chrome helpers become one "Google Chrome" group)
4. Agent-runtime scanning groups Codex, Claude, Cursor, Windsurf, Zed, MCP servers, renderers, tool workers, and stale processes into workflow impact
5. Workload advice converts hardware and runtime pressure into concrete actions: run, degrade, or defer
6. Multi-signal scoring (RAM + CPU + swap) classifies system health into 4 tiers: Healthy, Degrading, Strained, Critical

## CLI Commands

```bash
axon serve              # Start MCP stdio server (default, used by agents)
axon serve --dashboard  # Start local dashboard at http://127.0.0.1:7670
axon diagnose           # One-shot: collect 4s of data, print the culprit
axon status             # Print current hardware snapshot as JSON
axon query <tool>       # Call an MCP tool directly (e.g., axon query agent_runtime_health)
axon setup <target>     # Configure an agent (claude-desktop, claude-code, cursor, vscode)
```

Useful direct queries:

```bash
axon query agent_runtime_health
axon query workload_advice
axon query process_blame
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
  axon-core/     # Types, EWMA tracker, impact engine, agent runtime scanner, collector loop, alert dispatch, SQLite persistence
  axon-server/   # MCP server (9 tools via rmcp)
  axon-cli/      # Binary entry point
  dashboard/     # Local zero-cloud dashboard
```

Key design decisions:
- **Privacy by architecture** -- no network calls, no telemetry, no cloud
- **stdio transport** -- universal MCP compatibility with all current agents
- **SQLite persistence** -- snapshots every 10s, alerts on state transitions, powers trend queries
- **Edge-triggered alerts** -- fire once on state transitions (Normal->Warn, Healthy->Strained), not on every tick
- **Business-impact layer** -- converts raw hardware signals into task-level advice an agent can actually use

## Requirements

- macOS (Apple Silicon or Intel), Linux, and Windows.
- Rust 1.75+ (for building from source)

## See also

- [Public website](docs/index.html) -- GitHub Pages-ready landing page for Axon
- [The evidence: 15 GitHub issues and research](docs/problem-validation.md) -- why this problem exists
- [Agent platform demo](docs/agent-platform-demo.md) -- live local demo narrative for agent IDEs, app builders, and local agent runtimes
- [Value scorecard](docs/value-scorecard.md) -- metric-driven scorecard for cost, time, and performance impact
- [App-development A/B report](app_dev_ab_report.md) -- with/without Axon proof, including no-degradation checks
- [Agent adaptation test](agent_behavior_report.md) -- 50.3% latency reduction under stress
- [Parallel agent comparison](comparative_stress_test_results/comparison_report.md) -- blind vs informed agents on one machine

## License

MIT
