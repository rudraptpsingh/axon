# Blind vs Informed: What Happens When 4 Agents Fight for One Machine

Four agents. One machine. Build, test, analyze, data-process -- all at once.

In Scenario A, they are blind. They pile on simultaneously. CPU hits 100%. Average utilization during stress: 99.97%. This is where kernel panics happen.

In Scenario B, they can see. Each agent checks `hw_snapshot` before starting. They take turns. Average CPU during stress: 48.05%. Four alerts fire -- as signals, not emergencies. The machine survives.

---

## The issues this recreates

Every scenario below is a real bug report from developers using Claude Code:

- **[#15487](https://github.com/anthropics/claude-code/issues/15487)**: A developer ran 24 parallel sub-agents. I/O storm. System lockup. Nobody could use the machine until the processes were killed.

- **[#17563](https://github.com/anthropics/claude-code/issues/17563)**: Extreme CPU and RAM usage on Apple Silicon. MacBook Air M4 overheats. Battery life drops from 8 hours to 2. Thermal throttling degrades everything on the machine.

- **[#11122](https://github.com/anthropics/claude-code/issues/11122)**: Multiple Claude CLI processes accumulate silently across sessions. The developer reports "my computer is slow" without realizing 5 instances of Claude are running.

- **[#4850](https://github.com/anthropics/claude-code/issues/4850)**: Sub-agents spawn sub-agents in an endless loop. Memory climbs until OOM kills everything.

- **[#21403](https://github.com/anthropics/claude-code/issues/21403)**: 15-17GB memory consumption with parallel sub-agents on a 16GB machine. OOM kill.

- **[#33963](https://github.com/anthropics/claude-code/issues/33963)**: OOM crash. No self-monitoring. No graceful degradation. The agent runs until the system dies.

- **[#4580](https://github.com/anthropics/claude-code/issues/4580)**: 100% CPU freeze during multi-agent task serialization. The machine becomes unresponsive.

The common thread: the agents had no way to know they were destroying the machine. They kept going.

---

## Scenario A: Blind

All four agents launch their tasks simultaneously. Nobody checks the system first.

| Agent | Task | Duration |
|-------|------|----------|
| Build Agent | Compile project (CPU-heavy) | 120s |
| Test Agent | Run test suite (CPU + RAM) | 120s |
| Analysis Agent | Codebase analysis (I/O + CPU) | 120s |
| Data Agent | Process large dataset (RAM-heavy) | 120s |

The machine hits a wall. CPU averages 99.97% for the entire stress window. RAM peaks at 51.66%. On a 16GB laptop -- the kind most developers use -- this is the zone where macOS Jetsam starts killing processes and Linux OOM killer steps in.

All four tasks completed in this test. On a machine with less headroom, some of them would not have.

## Scenario B: Informed

Same four agents. Same four tasks. One difference: each agent queries Axon's `hw_snapshot` before starting and every 30 seconds during execution.

The Build Agent goes first. It checks headroom, sees `limited`, and proceeds with monitoring. When it finishes, the Test Agent checks -- system is clear, it proceeds. The Analysis Agent and Data Agent follow the same pattern.

Average CPU during the stress window: 48.05%. Average RAM: 10.73%. The machine has room to breathe.

Four alerts fired during the run:

| Time | Alert | Severity |
|------|-------|----------|
| 10:01:24 | Disk pressure elevated (89% used) | Warning |
| 10:05:38 | Disk pressure critical (91% used) | Critical |
| 10:08:20 | RAM pressure elevated (9.0/16GB) | Warning |
| 10:08:34 | RAM pressure critical (12.0/16GB) | Critical |

These alerts were signals that the agents received and responded to. Not crashes. Not panics. Signals.

---

## The decision log

The agents made 17 Axon-informed decisions during Scenario B. Three key moments:

At 10:03:26, the Build Agent finishes and reports: `headroom=limited, alerts=1`. The system is warm but stable. Five seconds later, the Test Agent checks and sees `headroom=limited, impact=healthy` -- safe to proceed.

At 10:07:37, the Analysis Agent completes and reports: `headroom=insufficient, alerts=2`. Two alerts have fired. The Data Agent waits 6 seconds, checks again, sees `headroom=limited, impact=healthy`, and proceeds.

At 10:08:43, the Data Agent sees `headroom=insufficient, impact=degrading` mid-task. It continues (the task is already running) but the signal is there -- if this were a decision point, it would defer.

Full decision log:

| Time | Agent | Decision | Reason |
|------|-------|----------|--------|
| 10:01:26 | Build Agent | proceed | headroom=limited, impact=healthy |
| 10:01:56 | Build Agent | continue | headroom=limited, impact=degrading |
| 10:02:26 | Build Agent | continue | headroom=limited, impact=degrading |
| 10:02:56 | Build Agent | continue | headroom=limited, impact=degrading |
| 10:03:26 | Build Agent | assessment | headroom=limited, alerts=1 |
| 10:03:31 | Test Agent | proceed | headroom=limited, impact=healthy |
| 10:04:01 | Test Agent | continue | headroom=limited, impact=degrading |
| 10:04:32 | Test Agent | continue | headroom=limited, impact=degrading |
| 10:05:02 | Test Agent | continue | headroom=limited, impact=degrading |
| 10:05:32 | Test Agent | assessment | headroom=limited, alerts=1 |
| 10:05:37 | Analysis Agent | proceed | headroom=limited, impact=healthy |
| 10:07:37 | Analysis Agent | assessment | headroom=insufficient, alerts=2 |
| 10:07:43 | Data Agent | proceed | headroom=limited, impact=healthy |
| 10:08:13 | Data Agent | continue | headroom=limited, impact=healthy |
| 10:08:43 | Data Agent | continue | headroom=insufficient, impact=degrading |
| 10:09:13 | Data Agent | continue | headroom=insufficient, impact=degrading |
| 10:09:46 | Data Agent | assessment | headroom=limited, alerts=4 |

---

## The takeaway

Axon did not make the agents faster. It made them aware.

They chose to go one at a time because they could see the cost of going all at once. No central scheduler. No orchestration layer. No rate limiter. Just a shared view of reality -- CPU, RAM, disk, temperature -- and agents smart enough to use it.

This is the difference between an agent that crashes your machine and one that respects it.

---

## Get axon

```bash
brew install rudraptpsingh/tap/axon
axon setup   # configures all detected agents
```

Details in the [README](../README.md). Single-agent adaptation test in [agent_behavior_report.md](../agent_behavior_report.md). Full evidence in [problem-validation.md](../docs/problem-validation.md).
