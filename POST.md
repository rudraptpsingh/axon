# axon -- your AI coding agent's missing sense

Your AI coding agent can write code, search files, run tests. But it has no idea your Mac is about to melt.

I've been using Claude, Cursor, and VS Code Copilot daily for months. The pattern is always the same: you're deep in a session, the agent is firing off builds and edits, and suddenly everything lags. The fan spins up. Cursor freezes. You force-quit, restart, lose context, and start over.

The agent never knew it was killing your machine. It had no way to know.

axon fixes that.

---

## What it actually does

axon is an MCP server that runs locally on your Mac. It gives any compatible AI agent (Claude Desktop, Cursor, VS Code, Claude Code) real-time awareness of your hardware state. No cloud. No telemetry. Nothing leaves your machine.

When your agent calls `process_blame`, it gets back something like this:

```json
{
  "ok": true,
  "data": {
    "anomaly_type": "memory_pressure",
    "impact_level": "strained",
    "culprit": {
      "pid": 1234,
      "cmd": "Cursor",
      "cpu_pct": 210.0,
      "ram_gb": 13.8,
      "blame_score": 0.87
    },
    "anomaly_score": 0.72,
    "impact": "System is slowing down. Applications may lag or become unresponsive.",
    "fix": "Restart Cursor or close unused tabs (Cmd+W)."
  },
  "narrative": "Cursor (PID 1234, 210% CPU, 13.8GB RAM) -- System is slowing down. Applications may lag or become unresponsive. Fix: Restart Cursor or close unused tabs (Cmd+W)."
}
```

The agent can read that, understand the situation, and act on it -- throttle its own activity, suggest the user close something, or pause a heavy build.

---

## The four tools

axon exposes four MCP tools over stdio:

**process_blame** -- the hero tool. Identifies the single process most responsible for system strain, scores its impact, and suggests a specific fix. Uses per-process EWMA baselines to detect anomalous spikes, not just raw usage.

**hw_snapshot** -- raw hardware state at a glance:

```json
{
  "ok": true,
  "data": {
    "die_temp_celsius": 46.3,
    "throttling": false,
    "ram_used_gb": 6.26,
    "ram_total_gb": 8.0,
    "ram_pressure": "warn",
    "cpu_usage_pct": 19.8
  },
  "narrative": "CPU 19.8%, RAM 6.3/8.0GB (warn), Temp 46C"
}
```

**battery_status** -- percentage, charging state, time remaining:

```json
{
  "ok": true,
  "data": {
    "percentage": 99.0,
    "is_charging": true,
    "time_to_empty_min": null,
    "narrative": "Battery at 99% and charging."
  }
}
```

**system_profile** -- machine identity, cached at startup:

```json
{
  "ok": true,
  "data": {
    "model_id": "Mac14,2",
    "chip": "Apple Silicon",
    "core_count": 8,
    "ram_total_gb": 8.0,
    "os_version": "macOS 15.4.1",
    "axon_version": "0.1.0"
  },
  "narrative": "Mac14,2, Apple Silicon, 8 cores, 8GB RAM, macOS 15.4.1"
}
```

---

## CLI output

You don't need an agent to use axon. The CLI works standalone.

`axon diagnose` collects 4 seconds of data and prints the verdict:

```
[warn] Claude Helper (PID 71837)  --  8% CPU,  0.0GB RAM
       Impact: System is healthy. No action needed.
       Fix:    Reduce system load by closing unused applications.
      Temp:   48C
      Battery: Battery at 99% and charging.
```

`axon status` dumps the current hardware snapshot as JSON:

```json
{
  "die_temp_celsius": 46.322174072265625,
  "throttling": false,
  "ram_used_gb": 6.2618865966796875,
  "ram_total_gb": 8.0,
  "ram_pressure": "warn",
  "cpu_usage_pct": 19.779577255249023,
  "ts": "2026-03-20T06:33:42.282242Z"
}
```

---

## The problem it solves

AI coding agents are powerful but blind. They have no concept of the physical machine they're running on. This creates real problems:

**Runaway builds.** The agent kicks off `cargo build` with max parallelism on an 8GB MacBook Air. RAM fills up, swap hits, everything crawls. The agent doesn't know. It just waits, retries, or keeps piling on.

**Thermal throttling.** A long session with multiple tool calls heats the CPU past 95C. The chip throttles itself to avoid damage. Everything gets 3x slower. The agent keeps working at normal pace, queuing up more work on a machine that can barely keep up.

**Battery drain.** You're on battery at a coffee shop. The agent is happily running builds and tests. Your battery is at 12% with 38 minutes left. The agent has no idea.

**Memory pressure.** Cursor itself is using 14GB of RAM. The system is paging constantly. The agent's next action will probably crash the app. But it doesn't know that.

axon gives the agent the information it needs to be a good citizen on your machine. It's the difference between a coworker who cranks the stereo while you're on a call, and one who checks first.

---

## How it works under the hood

A background collector refreshes hardware state every 2 seconds via `sysinfo`. For each process, it maintains an EWMA (Exponentially Weighted Moving Average) baseline with alpha=0.2 -- roughly a 5-sample rolling window.

When a process spikes above its own baseline, the delta is captured and weighted:
- Memory-pressure anomalies weight RAM delta at 75%, CPU at 25%
- CPU/thermal anomalies weight CPU at 60%, RAM at 40%
- General slowdowns split 50/50

A persistence filter requires 3+ consecutive anomalous samples before escalating. This avoids false positives from transient spikes -- a brief `cargo test` compilation shouldn't trigger a warning, but sustained resource hogging should.

The system-level anomaly score combines RAM usage (40%), CPU usage (30%), and swap pressure (30%) into a single value mapped to four tiers: healthy, degrading, strained, critical.

Known resource hogs (Cursor, cargo, node, Docker, Ollama, VS Code) get specific fix suggestions from a hardcoded table. Unknown processes get a generic recommendation.

---

## Zero setup

axon auto-configures itself for Claude Desktop, Cursor, and VS Code on first run. Install it, restart your agent, and it just works.

```bash
# Homebrew (recommended)
brew tap rudraptpsingh/axon
brew install axon

# Or from source
cargo install --path crates/axon-cli
```

Restart Claude Desktop / Cursor / VS Code. axon writes the MCP config entries automatically.

Or set up a specific agent manually:

```bash
axon setup claude-desktop
axon setup claude-code
axon setup cursor
axon setup vscode
```

---

## Privacy by architecture

axon makes zero network calls. There is no telemetry, no analytics, no cloud backend. The binary reads your hardware sensors and talks to your agent over stdio. Data never leaves your machine. This isn't a policy decision -- it's an architecture decision. There's literally no networking code in the binary.

---

## What's next

axon is an MVP. The core loop works: detect, blame, suggest. The immediate focus is stability and real-world validation.

On the roadmap: GPU attribution for Metal workloads, cross-session memory via SQLite, Linux support, predictive alerts ("your RAM will hit critical in ~10 minutes"), and process grouping (blaming Chrome as a whole, not individual helper processes).

But first: ship, dogfood, listen.

---

**Links:**
- GitHub: https://github.com/rudraptpsingh/axon
- License: MIT
- Requirements: macOS (Apple Silicon or Intel), Rust 1.75+ for building from source
