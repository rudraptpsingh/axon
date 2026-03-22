# The Evidence: AI Coding Agents vs Your Hardware

A developer left 4 Claude Code sessions idle overnight. By morning, they had consumed 60GB of RAM. The macOS OOM killer fired. Full system crash. No warning. No signal. No self-regulation.

[Issue #18859](https://github.com/anthropics/claude-code/issues/18859).

This is not a one-off. It is a pattern. AI coding agents -- Claude Code, Cursor, VS Code with Copilot -- operate completely blind to the hardware they run on. They have no CPU meter, no RAM gauge, no temperature reading. When your machine is dying, they keep going. They have no way to know.

This page collects the evidence: real crash reports, academic research, and industry data. If you have experienced any of these problems, [axon](../README.md) exists to fix them.

---

## "My machine crashed"

These are not edge cases. These are developers losing work because their AI agent ate the machine alive.

**[Issue #24960](https://github.com/anthropics/claude-code/issues/24960)** -- 3 Claude processes hit 17.3GB on an 18GB machine. Fan noise was the only warning. Kernel panic. Forced power-off. Watchdog timeout after 93 seconds.

**[Issue #18859](https://github.com/anthropics/claude-code/issues/18859)** -- 4 idle Claude Code sessions accumulated 60GB total over 18 hours with no user interaction. macOS Jetsam OOM killer triggered. Full system crash.

**[Issue #11377](https://github.com/anthropics/claude-code/issues/11377)** -- After 14 hours of runtime, a single Claude process consumed 23GB RAM and 143% CPU. The system became completely unresponsive.

In every case, the agent continued issuing tool calls, spawning processes, and running tasks while the machine was failing. There was no interrupt. No signal. No self-regulation.

---

## "My laptop is on fire"

Thermal throttling is invisible to the agent. It keeps pushing while the hardware slows everything down.

**[Issue #17563](https://github.com/anthropics/claude-code/issues/17563)** -- MacBook Air M4 overheats during Claude Code sessions. Battery life drops from 8 hours to 2. Thermal throttling degrades all work on the machine, not just the agent's tasks.

**[Issue #11615](https://github.com/anthropics/claude-code/issues/11615)** -- Code Helper (Renderer) consuming 99.8% CPU during Claude Code sessions. Affects VS Code and Cursor simultaneously.

**[Issue #9897](https://github.com/anthropics/claude-code/issues/9897)** -- "Claude Code is using massive amounts of memory and heating up my computer." The developer noticed. The agent did not.

---

## "Everything is slow and I do not know why"

The most insidious failure mode. The machine degrades gradually. The developer feels it but cannot pinpoint it. The agent has no idea.

**[Issue #11122](https://github.com/anthropics/claude-code/issues/11122)** -- Multiple Claude CLI processes accumulate across sessions, compounding CPU usage. Developers report "computer is slow" without realizing multiple instances are running.

**[Issue #17148](https://github.com/anthropics/claude-code/issues/17148)** -- Claude Code consumes 100%+ CPU while idle due to a busy-poll loop (`setImmediate()` calls without yielding). The agent is doing nothing useful but the machine is maxed out.

---

## What the research says

The problems above are not unique to Claude Code. They reflect a structural gap in how AI coding tools interact with hardware.

**METR randomized controlled trial** (2025): Experienced open-source developers using AI tools took 19% longer to complete tasks than without. Before the study, they predicted AI would make them 24% faster. After experiencing the slowdown, they still estimated a 20% improvement -- a complete inversion of reality. Much of the gap comes from agents and developers spending time gathering context that should be automatic.
[metr.org](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/)

**Qodo "State of AI Code Quality 2025"**: 65% of developers cite missing context as the top failure mode during refactoring and test generation. "Missing context" includes system context -- the agent has no idea the machine is under load.
[qodo.ai](https://www.qodo.ai/reports/state-of-ai-code-quality/)

**IEEE Spectrum "AI Coding Degrades"**: Flaky tests are increasingly misattributed to race conditions or concurrency bugs when the actual cause is memory pressure mid-test. Wrong diagnosis, wasted context, wrong fix.
[spectrum.ieee.org](https://spectrum.ieee.org/ai-coding-degrades)

**"Your AI Coding Assistant Just Moved the Bottleneck"** (Michael Henderson): AI accelerates code generation but exposes new bottlenecks in hardware, review, and CI. The bottleneck does not disappear -- it moves.
[medium.com](https://medium.com/@michaelhenderson/your-ai-coding-assistant-just-moved-the-bottleneck-and-made-it-worse-3780f1c415eb)

---

## The token cost

Without axon, an agent diagnosing slowness runs `ps aux | sort -rk 3 | head -20`, `top -l 1 | head -30`, `vm_stat`, parses free-form text, then reasons about it. That is approximately 2,000-3,000 tokens of confused exploration producing an approximate answer.

With axon's `process_blame` tool, the agent gets a structured JSON response in ~200 tokens: anomaly type, impact level, culprit process with PID and resource usage, and a specific fix string. 10-15x token reduction. Deterministic output.

---

## What axon does about it

Axon is an [MCP](https://modelcontextprotocol.io/) server that gives agents real-time hardware awareness. Zero network calls -- your process names, load patterns, and hardware state never leave your machine.

| Symptom | Axon tool | What the agent gets |
|---------|-----------|-------------------|
| Machine crashes (OOM, kernel panic) | Edge-triggered RAM alerts | Immediate notification on state transition (Normal to Warn to Critical) |
| Thermal throttling, overheating | `hw_snapshot` | Throttle flag, die temperature, headroom level |
| Slow performance misdiagnosed as code | `process_blame` | Culprit process, anomaly type, impact level, specific fix string |
| Context burned on shell diagnostics | `process_blame` | Structured JSON in 200 tokens vs 3,000 tokens of shell parsing |
| Flaky tests blamed on code | `hardware_trend` | Anomaly count and resource peaks correlated with test window |
| No interrupt during long sessions | Edge alerts + `session_health` | Worst state, alert count, peak CPU/RAM/temp since session start |
| Accumulated agent processes | `process_blame` | Detects N instances of Claude/Cursor/Windsurf running simultaneously |

Install axon, run `axon setup`, and your agent gets hardware eyes. Details in the [README](../README.md).

See the test results: [single-agent adaptation](../agent_behavior_report.md) (50.3% latency reduction) and [parallel agent comparison](../comparative_stress_test_results/comparison_report.md) (blind vs informed).
