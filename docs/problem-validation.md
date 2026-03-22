# Problem Validation: Why AI Coding Agents Need Hardware Awareness

This document records externally validated evidence for the problems axon solves.
Sources are GitHub issues, academic studies, developer blogs, and industry reports
gathered March 2026.

---

## The Core Gap

AI coding agents (Claude Code, Cursor, VS Code with Copilot) operate completely
blind to the hardware they run on. They have no native mechanism to observe CPU
load, RAM pressure, die temperature, or thermal throttling state. They cannot
self-regulate, pre-check headroom, or correlate task failures with system state.

This is not a theoretical gap. It is causing documented, reproducible failures
right now.

---

## Problem 1: Agents actively cause hardware failures and keep going

### Evidence

The `anthropics/claude-code` GitHub repository contains a sustained series of
confirmed bugs, all sharing the same root pattern: the agent has no visibility
into what it is doing to the machine.

- **Issue #17563** -- "Extreme CPU/RAM usage and thermal throttling on Apple
  Silicon since v2.1.4." MacBook Air M4 overheats. Battery life drops from 8h
  to 2h. Thermal throttling degrades all work on the machine.
  https://github.com/anthropics/claude-code/issues/17563

- **Issue #11615** -- "Code Helper (Renderer) consuming 99.8% CPU during Claude
  Code sessions." Affects VS Code and Cursor IDE simultaneously.
  https://github.com/anthropics/claude-code/issues/11615

- **Issue #9897** -- "Claude Code is using massive amounts of memory and heating
  up my computer."
  https://github.com/anthropics/claude-code/issues/9897

- **Issue #11377** -- After 14 hours of runtime, a single Claude process
  consumed 23GB RAM and 143% CPU. System became completely unresponsive.
  https://github.com/anthropics/claude-code/issues/11377

- **Issue #18859** -- 4 idle Claude Code sessions accumulated 60GB total over
  18 hours with no user interaction. macOS Jetsam OOM killer triggered. Full
  system crash.
  https://github.com/anthropics/claude-code/issues/18859

- **Issue #24960** -- 3 Claude processes hit 17.3GB on an 18GB machine. Fan
  noise audible before crash. Kernel panic. Forced power-off. Watchdog timeout
  after 93 seconds.
  https://github.com/anthropics/claude-code/issues/24960

- **Issue #17148** -- Claude Code consumes 100%+ CPU while idle due to a
  busy-poll loop (`setImmediate()` calls without yielding).
  https://github.com/anthropics/claude-code/issues/17148

- **Issue #11122** -- Multiple Claude CLI processes accumulate across sessions,
  compounding CPU usage. Users report "computer is slow" without realizing
  multiple instances are running.
  https://github.com/anthropics/claude-code/issues/11122

### The core problem

In every case above, the agent continued issuing tool calls, spawning processes,
and running tasks while the machine was failing. There was no interrupt. No
signal. No self-regulation. The agent had no way to know.

### How axon addresses this

- `hw_snapshot` exposes RAM pressure (Normal/Warn/Critical), die temperature,
  and thermal throttle state. An agent can check before starting heavy tasks.
- Edge-triggered alerts fire the moment RAM or impact thresholds are crossed,
  delivered via MCP `notifications/message` or webhook -- no polling required.
- An agent that receives a Critical RAM alert or throttle-onset notification
  can stop issuing tasks, warn the user, and wait.

### Gap this reveals

Axon provides the data and the alert. But agents need a documented behavioral
contract: what to do when an alert fires during an active task. This is an
agent-side prompting and system-prompt problem, not a tool problem. The
`README.md` "For AI Agents" section partially covers this but could be more
explicit about mid-task interruption behavior.

---

## Problem 2: Agents misdiagnose slow performance as code problems

### Evidence

- METR conducted a randomized controlled trial with experienced open-source
  developers. Developers using AI tools took **19% longer** to complete tasks
  than without. Before the study, the same developers predicted AI would make
  them 24% faster. After experiencing the slowdown, they still estimated AI
  had improved their productivity by 20% -- a complete inversion of reality.
  https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/

  The Register coverage:
  https://www.theregister.com/2025/07/11/ai_code_tools_slow_down/

- "Your AI Coding Assistant Just Moved the Bottleneck" (Medium, Michael
  Henderson): AI accelerates code generation but exposes new bottlenecks --
  in hardware, in review, in CI. The bottleneck does not disappear, it moves.
  https://medium.com/@michaelhenderson/your-ai-coding-assistant-just-moved-the-bottleneck-and-made-it-worse-3780f1c415eb

- "Why Most AI Coding Tools Fail" (DEV Community): When builds are slow or
  tests fail intermittently, agents chase code-level fixes -- refactoring,
  adding retries, adjusting timeouts -- when the actual cause is machine
  saturation. Wrong diagnosis, wasted context, wrong fix.
  https://dev.to/lofcz/why-most-ai-coding-tools-fail-and-how-they-succeed-i31

### How axon addresses this

`process_blame` returns a structured `AnomalyType` (MemoryPressure /
CpuSaturation / ThermalThrottle / GeneralSlowdown) with a culprit process and
a specific fix string. One tool call replaces what would otherwise be a multi-
step shell diagnostic loop producing the wrong conclusion.

---

## Problem 3: Context window burned on manual hardware diagnosis

### Evidence

- Qodo "State of AI Code Quality 2025": 65% of developers cite missing context
  as the top failure mode during refactoring and test generation. "Missing
  context" includes system context -- the agent has no idea the machine is
  under load.
  https://www.qodo.ai/reports/state-of-ai-code-quality/

- "Why Is Claude Code So Slow Today?" (usagebar.com): Developers are manually
  checking Activity Monitor to find the issue. The agent has no equivalent
  built-in capability.
  https://usagebar.com/blog/why-is-claude-code-slow-today

- Cerbos "Productivity Paradox of AI Coding Assistants": Much of the observed
  slowdown in the METR study comes from cognitive overhead -- agents and the
  developers supervising them spending time gathering context that should be
  automatic.
  https://www.cerbos.dev/blog/productivity-paradox-of-ai-coding-assistants

### The specific token cost

Without axon, an agent diagnosing slowness runs:
`ps aux | sort -rk 3 | head -20`, `top -l 1 | head -30`, `vm_stat`, parses
free-form text, then reasons about it. That is approximately 2,000-3,000 tokens
of confused exploration producing an approximate answer.

With `process_blame`, the agent gets a structured JSON response in ~200 tokens
with anomaly type, impact level, culprit PID/cmd/cpu/ram, process group
totals, and a fix string. 10-15x token reduction. Deterministic output.

---

## Problem 4: Flaky tests blamed on code, not resource exhaustion

### Evidence

- IEEE Spectrum "AI Coding Degrades": Flaky tests are a leading complaint.
  They are increasingly misattributed to race conditions or concurrency bugs
  when the actual cause is memory pressure mid-test.
  https://spectrum.ieee.org/ai-coding-degrades

- Testlio "AI Testing Fails 2025": Most AI testing failures in 2025 were not
  edge cases -- they were basic environment problems that tooling missed.
  https://testlio.com/blog/ai-testing-fails-2025/

- Qodo report: 65% of developers cite context gaps (not hallucinations) as the
  primary cause of poor AI output during testing workflows.
  https://www.qodo.ai/reports/state-of-ai-code-quality/

### How axon addresses this

`hardware_trend` lets an agent correlate test failures with hardware anomaly
counts over the same time window. "3 test failures occurred in a period where
anomaly_count=12 and CPU peaked at 97%" is a data-backed conclusion that
redirects the diagnosis away from code.

---

## Problem 5: No interrupt signal when conditions degrade mid-session

### Evidence

- Issue #11377: After 14 hours, Claude consumed 23GB RAM / 143% CPU. Nobody
  interrupted it. It ran until the system became unresponsive.
  https://github.com/anthropics/claude-code/issues/11377

- Issue #18859: 4 idle sessions accumulated 60GB over 18 hours with no user
  interaction. No signal was sent. No self-limiting occurred.
  https://github.com/anthropics/claude-code/issues/18859

- Cursor "slow pool" behavior documented at cursor-ide.com: After rate limit
  exhaustion, Cursor silently degrades response quality. No signal to the
  agent. It continues working at degraded speed without knowing why.
  https://www.cursor-ide.com/blog/cursor-claude-45-lag-fix

### How axon addresses this

Alerts are edge-triggered on state transitions (RAM Normal->Warn, throttle
onset, impact Healthy->Strained) and delivered without polling via MCP
`notifications/message` or webhook. An agent receives the notification the
moment conditions change.

See `CLAUDE.md` for the alert architecture: alerts are inserted into SQLite
in the collector loop, independent of any MCP connection.

---

## Problem 6: AI agents are architecturally blind to hardware

### Evidence

- Anthropic's MCP announcement: The explicit rationale for the Model Context
  Protocol is that AI models are "isolated from data" and "trapped behind
  information silos." This extends to local hardware -- no native mechanism
  exists for an agent to observe the machine it runs on.
  https://www.anthropic.com/news/model-context-protocol

- DEV Community "MCP Servers Explained": MCP is the mechanism to give agents
  "eyes and ears" into real-time system state. Without an MCP server providing
  hardware data, agents have no hardware visibility at all.
  https://dev.to/alchemic_technology/mcp-servers-explained-give-your-ai-agent-real-tools-not-just-chat-354

- Datadog MCP Server (2025): Even large cloud vendors now ship MCP servers
  specifically to give agents live observability data. The gap is recognized
  industry-wide. Axon is the local, privacy-preserving equivalent for
  developer workstations.
  https://www.datadoghq.com/product/ai/mcp-server/

### How axon addresses this

Axon is the MCP server that fills this gap for local hardware. It is the only
tool designed specifically to give AI coding agents real-time awareness of
CPU, RAM, thermals, disk, process load, and battery over the same stdio
transport Claude Code already uses.

---

## Problem 7: Cloud monitoring is not viable for developer workstations

### Evidence

- Zenity MCP Security Primer: Cloud-based monitoring requires sending process
  telemetry off-device. For developers working on proprietary code, process
  names, resource patterns, and timing data are sensitive.
  https://zenity.io/blog/current-events/model-context-protocol

- Bytebridge "MCP Gateways in 2026": Most MCP tooling is cloud-oriented. A
  gap exists for privacy-sensitive local tooling.
  https://bytebridge.medium.com/mcp-gateways-in-2026-top-10-tools-for-ai-agents-and-workflows-d98f54c3577a

### How axon addresses this

Zero network calls. Enforced as a hard design constraint (see `CLAUDE.md`: "No
network calls. This is a core design constraint. Never add telemetry, analytics,
or any outbound network activity."). All data stays local. Process names, load
patterns, and hardware state never leave the machine.

---

## Proposed Solutions / Feature Gaps Identified

The research above validates axon's existing tools but also reveals specific
gaps where axon's coverage could be extended.

### Gap 1: Detect accumulated agent processes

Issue #11122 documents a specific failure mode: multiple Claude CLI processes
accumulate across sessions, compounding CPU. Users report "computer is slow"
without knowing the cause. `process_blame` returns the top culprit, but does
not currently flag "N instances of Claude/Claude Code are running" as its own
anomaly type.

**Proposed:** Add recognition of common agent process names (claude, Claude
Code, cursor, Cursor Helper) in the process grouping logic, with a specific
fix suggestion when N>1 instances of the same agent are detected.

### Gap 2: Pre-task headroom check as a single boolean

`hw_snapshot` provides all the data needed to decide "can I start this heavy
task?" but the agent must interpret RAM pressure + disk pressure + CPU + temp
together. A dedicated `safe_to_proceed` field or tool that returns a boolean
plus a short reason would reduce agent-side reasoning overhead for the most
common agentic use case.

**Proposed:** Add a `headroom` field to the `hw_snapshot` response: a simple
Adequate/Limited/Insufficient classification with a one-line reason string.
No new tool needed -- just an additional field in the existing snapshot.

### Gap 3: Long-session periodic health summary

Issues #11377 and #18859 both involve multi-hour sessions where gradual
degradation went unnoticed. Edge-triggered alerts cover threshold crossings
but do not provide a periodic "system health since session start" summary.

**Proposed:** A session-health tool or a `session_summary` field in
`hardware_trend` that returns the worst state seen since a given timestamp,
number of alerts fired, and average anomaly score. This gives agents context
for multi-hour sessions without requiring them to know the exact alert history.

### Gap 4: Linux and Windows support

Issues are not macOS-exclusive. The underlying `sysinfo` crate supports Linux
and Windows. As Claude Code and Cursor are heavily used on Linux dev machines
and Windows laptops, the same problems occur there. This is already noted as
planned in `CLAUDE.md` and `README.md`.

---

## Summary

| Problem | External evidence | Axon tool | Gap |
|---|---|---|---|
| Agent causes OOM / kernel panic | GitHub #24960, #18859, #11377 | Edge-triggered RAM alerts | Agent behavioral contract for mid-task interrupt |
| Thermal throttling, overheating | GitHub #17563, #11615 | `hw_snapshot` throttle flag | None -- fully covered |
| Slow performance misdiagnosed as code | METR study, DEV Community | `process_blame` | None -- fully covered |
| Context burned on shell diagnostics | Qodo report, usagebar | `process_blame` structured JSON | None -- fully covered |
| Flaky tests blamed on code | IEEE Spectrum, Testlio | `hardware_trend` anomaly correlation | None -- fully covered |
| No mid-session interrupt signal | GitHub #11377, #18859 | Edge alerts via MCP/webhook | Agent behavioral contract |
| Agents blind to hardware by design | Anthropic MCP docs, Datadog | Axon is the hardware MCP server | Linux/Windows support |
| Cloud monitoring leaks process data | Zenity, Bytebridge | Local-only, no network calls | None -- core design |
| Accumulated agent processes | GitHub #11122 | `process_blame` (partial) | Explicit multi-instance detection |
| Pre-task headroom check | GitHub #17563, #9897 | `hw_snapshot` (requires interpretation) | `headroom` field in snapshot |
| Long-session health summary | GitHub #11377, #18859 | `hardware_trend` (partial) | Session-scoped summary tool or field |
