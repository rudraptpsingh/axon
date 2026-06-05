# Axon

[![CI](https://github.com/rudraptpsingh/axon/actions/workflows/ci.yml/badge.svg)](https://github.com/rudraptpsingh/axon/actions/workflows/ci.yml)
[![Pages](https://github.com/rudraptpsingh/axon/actions/workflows/pages.yml/badge.svg)](https://github.com/rudraptpsingh/axon/actions/workflows/pages.yml)

Axon is a zero-cloud MCP server that gives local coding agents hardware and
runtime context before they start more work.

It runs on the developer's machine, exposes structured MCP tools over stdio,
and reports signals such as CPU pressure, RAM pressure, thermal throttling,
GPU state, stale agent sessions, duplicate MCP servers, and safe parallelism.

The goal is not to be a prettier system monitor. The goal is to let an agent
decide whether to run, reduce parallelism, defer, reuse existing tools, or ask
the user to clean up the local environment.

- Public site: <https://rudraptpsingh.github.io/axon/>
- Source: <https://github.com/rudraptpsingh/axon>
- License: [MIT](LICENSE)

## Why

Local coding agents often operate without knowing whether the host machine is
already saturated. That leads to predictable failure modes:

- slow builds misdiagnosed as code problems
- parallel test or browser runs launched on a pressured machine
- stale MCP servers and helper processes accumulating across sessions
- long-running agent sessions growing memory without a clear warning
- thermal throttling and battery pressure that the agent cannot see

Axon gives the agent a compact, structured answer instead of forcing it to run
shell diagnostics and parse free-form output.

## Privacy Boundary

Axon is local-first by design:

- no telemetry
- no analytics
- no automatic outbound network calls
- stdio transport for MCP clients
- local SQLite persistence for snapshots and alert history

Hardware and process data stay on the machine unless the user explicitly
exports or forwards it.

## Install

Homebrew:

```bash
brew install rudraptpsingh/tap/axon
```

From source:

```bash
cargo install --path crates/axon-cli
```

Requirements:

- macOS, Linux, or Windows
- Rust 1.75+ when building from source

## Configure Agents

Run setup after installing:

```bash
axon setup              # configure detected agents
axon setup claude-code
axon setup cursor
axon setup vscode
```

Manual MCP configuration:

```json
{
  "mcpServers": {
    "axon": {
      "command": "/absolute/path/to/axon",
      "args": ["serve"]
    }
  }
}
```

Restart the agent after changing MCP configuration.

## Quick Check

Run a one-shot diagnosis:

```bash
axon diagnose
```

Call an MCP tool directly:

```bash
axon query hw_snapshot
axon query process_blame
axon query workload_advice
axon query agent_runtime_health
```

Example response shape:

```json
{
  "ok": true,
  "ts": "2026-06-05T07:00:00Z",
  "data": {
    "headroom": "limited"
  },
  "narrative": "RAM pressure is elevated. Cap parallelism before starting heavy local work."
}
```

All MCP tools return JSON with an `ok` flag, timestamp, data payload, and a
human-readable `narrative`.

## MCP Tools

| Tool | Purpose |
| --- | --- |
| `hw_snapshot` | Current CPU, RAM, disk, thermal, swap, and headroom state. |
| `process_blame` | Top process or process group causing local pressure, with impact and fix guidance. |
| `battery_status` | Battery percentage, charging state, and estimated time remaining. |
| `system_profile` | Machine model, chip, core count, RAM, OS, and Axon version. |
| `hardware_trend` | CPU, RAM, temperature, pressure, and anomaly trends over time. |
| `session_health` | Alert count, worst impact, and resource peaks since a timestamp. |
| `gpu_snapshot` | GPU detection, utilization, VRAM state, and GPU-specific narrative. |
| `workload_advice` | Run/degrade/defer policy and safe parallelism for local work. |
| `agent_runtime_health` | Inventory of local agent processes, MCP servers, stale sessions, and duplicate tool groups. |

Suggested agent policy:

- call `system_profile` once at session start
- call `hw_snapshot` before heavy local work
- call `workload_advice` before builds, tests, Docker, browser automation,
  subagents, or GPU work
- call `agent_runtime_health` before spawning additional MCP servers or browser
  controllers
- call `process_blame` when the system feels slow or a local task stalls
- call `session_health` near the end of long sessions

## CLI

```bash
axon serve              # start MCP stdio server
axon serve --dashboard  # start local dashboard at http://127.0.0.1:7670
axon diagnose           # collect a short sample and print the likely culprit
axon status             # print current hardware snapshot as JSON
axon query <tool>       # call an MCP tool directly
axon setup <target>     # configure claude-desktop, claude-code, cursor, or vscode
```

## Alerts

Axon can emit edge-triggered alerts when system state changes, for example when
RAM pressure escalates or thermal throttling starts. Alerts are not periodic
pings.

Webhook example:

```bash
axon serve --alert-webhook myapp=https://example.com/alerts
```

Config file location:

```text
~/.config/axon/alert-dispatch.json
```

Set `AXON_CONFIG_DIR` to load `alert-dispatch.json` from a different directory,
or pass `axon serve --config-dir <dir>`.

## Architecture

```text
crates/
  axon-core/     # data types, collector, EWMA baselines, impact engine, persistence
  axon-server/   # MCP server, tool handlers, response narratives
  axon-cli/      # serve, diagnose, status, setup, query
dashboard/       # local zero-cloud dashboard
docs/            # public website and engineering notes
```

Key implementation details:

- collector refreshes hardware/process state every 2 seconds
- per-process EWMA baselines reduce false positives from transient spikes
- process grouping collapses helper processes into useful app-level blame
- agent-runtime scanner detects accumulated Codex, Claude, Cursor, Zed,
  Windsurf, MCP server, renderer, and tool-worker processes
- SQLite stores local snapshots and alerts for trend/session queries
- stdout is reserved for MCP JSON-RPC on the server path; logs go to stderr

## Development

```bash
cargo build
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

Useful smoke checks:

```bash
cargo run -p axon -- diagnose
cargo run -p axon -- query hw_snapshot
cargo run -p axon -- query workload_advice
python3 scripts/mcp_exercise_all_tools.py target/debug/axon
```

## Project Materials

- [Public website](https://rudraptpsingh.github.io/axon/)
- [Problem validation](docs/problem-validation.md)
- [App-development A/B analysis](docs/app-development-ab-analysis.md)
- [Agent platform demo](docs/agent-platform-demo.md)
- [Value scorecard](docs/value-scorecard.md)
- [Roadmap](docs/roadmap.md)
- [Testing guide](docs/testing-guide.md)

## Contributing

Contributions are welcome when they preserve the local privacy boundary. Start
with [CONTRIBUTING.md](CONTRIBUTING.md), keep changes scoped, and include tests
when behavior changes.

Security reports should follow [SECURITY.md](SECURITY.md). Please do not open
public issues for vulnerabilities.

## License

MIT. See [LICENSE](LICENSE).
