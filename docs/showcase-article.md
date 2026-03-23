# I Wasted 12 Minutes Debugging What My Tools Already Knew

I had `process_blame` and `hw_snapshot` sitting right there in my MCP toolbar. Available the entire session. Instead, I reached for `ps aux`.

That is the story of axon. Not the tool itself — the story of why it exists.

---

## The 3 AM enterprise incident that started this

Two years ago I was on-call for a platform team at a fintech. A data pipeline was flaking — jobs timing out, retries piling up, the usual 3 AM disaster. I SSH'd into the box, ran `top`, saw 98% CPU. Obvious culprit: a zombie Spark executor that should have been killed 4 hours ago. Killed it. Pipeline recovered. Went back to sleep.

The next morning my manager asked why it took 40 minutes to resolve. I said I was debugging the pipeline logic. He said: "The CPU was at 98% the entire time. Why didn't you check that first?"

Because I didn't think to look. The pipeline was failing, so I debugged the pipeline. The machine was the problem, and I had no signal telling me that.

Fast forward to 2025. I'm vibe coding with Claude Code on my M4 MacBook Air. Running eval loops, spawning sub-agents, building and testing in parallel. My laptop fans are screaming. Tests are flaking. I open Activity Monitor and see five Claude processes eating 14GB on a 16GB machine. I've been debugging "flaky tests" for 20 minutes. The tests aren't flaky. My machine is drowning.

Same pattern. Different decade. Same blind spot.

---

## The problem is not debugging skill — it's signal

I'm not a bad engineer. You're not a bad engineer. The problem is that AI coding agents — Claude Code, Cursor, VS Code Copilot — operate completely blind to the hardware they run on. They have no CPU meter. No RAM gauge. No temperature reading. When your machine is dying, they keep going. They literally cannot know.

This is not theoretical. I collected the receipts:

- **[#24960](https://github.com/anthropics/claude-code/issues/24960)**: 3 Claude processes hit 17.3GB on an 18GB machine. Fan noise was the only warning. Kernel panic. Forced power-off.
- **[#18859](https://github.com/anthropics/claude-code/issues/18859)**: 4 idle Claude Code sessions accumulated 60GB total overnight. macOS OOM killer fired. Full system crash. Zero user interaction.
- **[#11122](https://github.com/anthropics/claude-code/issues/11122)**: Multiple Claude CLI processes silently accumulate across sessions. Developer reports "my computer is slow" without realizing 5 instances are running.
- **[#15487](https://github.com/anthropics/claude-code/issues/15487)**: 24 parallel sub-agents create an I/O storm. System lockup. Nobody can use the machine.
- **[#33963](https://github.com/anthropics/claude-code/issues/33963)**: OOM crash. No self-monitoring. No graceful degradation. Agent runs until the system dies.

And the kicker — a [METR randomized controlled trial](https://metr.org/blog/2025-07-10-early-2025-ai-experienced-os-dev-study/) found developers using AI tools were **19% slower** than without them. But they *thought* they were 20% faster. The bottleneck moved to the hardware. Nobody noticed.

---

## So I built axon

axon is an MCP server. It runs locally, collects hardware state every 2 seconds, and exposes 7 tools that any AI agent can call. No cloud. No telemetry. No network calls — ever. That's not a config option, it's a hard architectural constraint.

```bash
brew install rudraptpsingh/tap/axon
axon setup   # configures Claude, Cursor, VS Code — all of them
```

One setup command. The agent restarts. Now it can see.

Here's what changed in my workflow:

**Before axon**: Build is slow. Agent runs `ps aux`, `top -l 1`, `vm_stat`, parses the output, guesses wrong, burns 2,000-3,000 tokens.

**After axon**: Agent calls `process_blame`. Gets back: "Cursor (2 processes, 204% CPU, 0.2GB RAM) — System is under load. Fix: restart Cursor or close unused tabs." 200 tokens. Done.

**Before axon**: Tests flake. Agent blames a race condition. Retries 3 times. Burns tokens. Still flaky.

**After axon**: Agent calls `hw_snapshot` before running tests. Sees `headroom: insufficient` — RAM at 73%, CPU saturated. Warns me: "System is under pressure, tests may be unreliable." The tests aren't broken. My machine is.

---

## The test that convinced me this actually works

I ran a controlled experiment. Four agents on one machine — Build, Test, Analysis, Data Processing — all launching simultaneously. The kind of thing that happens when you're deep in a vibe coding session with multiple sub-agents.

**Scenario A (blind):** All four pile on at once. CPU hits **99.97%**. RAM peaks at 51.66%. On a 16GB laptop, this is where the OOM killer lives.

**Scenario B (axon-aware):** Same four agents, same four tasks. Each checks `hw_snapshot` before starting. They take turns. CPU averages **48.05%**. RAM stays at **10.73%**. Four alerts fire — as signals, not emergencies. The machine survives.

The agents made 17 hardware-informed decisions during that run. Three moments stood out:

At 10:03, the Build Agent finishes and reports `headroom=limited, alerts=1`. The Test Agent checks, sees the system is warm but stable, proceeds. At 10:07, the Analysis Agent completes with `headroom=insufficient, alerts=2`. The Data Agent waits 6 seconds, checks again, sees recovery, then starts. At 10:08, it detects `headroom=insufficient, impact=degrading` mid-task — but the signal is there. If this were a decision point, it would defer.

No central scheduler. No orchestration layer. No rate limiter. Just a shared view of reality and agents smart enough to use it.

---

## The single-agent adaptation test

I also tortured a single agent. Gave it a queue to process, then set the machine on fire — 8 CPU stress processes, 60% RAM allocated, 4 disk I/O floods.

Without axon, P95 latency spiked **170%** (2.2ms to 5.8ms). The agent had no idea why. It kept processing the same way.

Then I gave it one tool call: `hw_snapshot` every 5 seconds. On the first query it saw `headroom: limited`. It switched from async to sync processing — a deliberate trade: throughput drops, but latency recovers **50.3%** (5.8ms down to 2.9ms). The queue stops growing. Memory stabilizes. The system stays responsive.

23 Axon queries logged. 58 sync-mode samples confirming the behavioral switch. Full recovery to baseline when stress ended.

The agent didn't get faster. It got aware. And awareness was enough.

---

## What's under the hood

I built this as a Rust workspace — three crates, minimal dependencies, runs on macOS and Linux:

- **axon-core**: The brain. EWMA baselines at three timescales (fast for spikes, medium for blame, slow for memory leaks). An impact engine that classifies system health into four tiers. Edge-triggered alerts that fire on state transitions, not every tick. Process grouping that turns 47 Chrome helpers into one "Google Chrome" entry. SQLite persistence for trends and alert history.

- **axon-server**: 7 MCP tools over stdio via rmcp. `hw_snapshot`, `process_blame`, `battery_status`, `system_profile`, `hardware_trend`, `session_health`, `gpu_snapshot`. Every response includes structured JSON and a human-readable narrative.

- **axon-cli**: `axon serve` for agents, `axon diagnose` for humans, `axon setup` for one-command configuration.

The test suite is... thorough. 12 test suites, 31 scripts, real hardware stress tests that spawn memory hogs and CPU stress processes. A live webhook E2E that runs `axon serve`, injects state transitions, captures HTTP POSTs, and validates payloads. An agent loop test that simulates three different agent strategies (reactive, proactive, monitoring) under real MCP connections. A 9-phase evaluation script that takes a release binary through baseline, stress, peak capture, cooldown, session health, trend analysis, GPU snapshot, webhook alerts, and lifecycle verification.

I tested this because I've been burned by tools that work in demos and break in production. This one works under load. That's the whole point.

---

## The 7 tools, quickly

| Tool | What it gives you | When to call it |
|------|------------------|-----------------|
| `hw_snapshot` | CPU, temp, RAM/disk pressure, throttling, **headroom** (adequate/limited/insufficient) | Before any heavy task |
| `process_blame` | Top culprit, anomaly type, impact level, specific fix command | When something is slow |
| `battery_status` | Charge %, charging state, time remaining | Before long-running work |
| `system_profile` | Machine model, chip, cores, RAM, OS | Session start |
| `hardware_trend` | CPU/RAM/temp averages over time | Correlating failures with load |
| `session_health` | Alert count, worst impact, peak metrics since timestamp | End of long sessions |
| `gpu_snapshot` | GPU util, VRAM, model, hang/reset count | Before GPU-heavy work |

All return `{ ok, ts, data, narrative }`. The `narrative` field is the human-readable version — agents can show it directly or use the structured data to make decisions.

---

## Why I'm writing this

Because last week I had `process_blame` and `hw_snapshot` available as MCP tools for my entire session. Instead of calling them first, I reached for `ps aux`. I wasted 3 eval cycles — about 12 minutes — debugging what axon already knew.

That's the problem axon solves. Not the technical problem of collecting metrics. The human problem of reaching for the wrong tool out of habit. And the agent problem of having no tool to reach for at all.

If you're running Claude Code, Cursor, or VS Code with AI — and you've ever wondered why your tests are flaky, your builds are slow, or your laptop sounds like a jet engine — your agent can't see what you can't see.

Give it eyes.

```bash
brew install rudraptpsingh/tap/axon
axon setup
```

---

*axon is open source, MIT licensed, and will never phone home. Your process names, load patterns, and hardware state stay on your machine. That's not a feature — it's the architecture.*
