---
name: axon
description: >-
  Local hardware and session intelligence for this machine. Use this skill during any
  coding session to avoid wasting tokens on work that will fail or stall: before starting
  heavy tasks (builds, tests, large edits, subagent fan-out), when the session feels slow
  or laggy, when memory/CPU/thermal pressure is suspected, or when the user asks how much
  axon has saved them. Runs fully on-device -- no data leaves the machine.
---

# axon -- hardware-aware, token-saving assistant

axon is a zero-cloud tool that tells you what the local machine is doing right now and
logs the tokens/cost it saves you. Drive it through the `axon` CLI.

> Reference copy. `axon setup claude-code-skill` installs a machine-specific version of
> this file at `~/.claude/skills/axon/SKILL.md` with the absolute binary path filled in.

- Binary: `axon` (installed copy uses the absolute path in case `axon` is not on PATH)

Every command prints JSON (for `query`) or a readable report (`savings`). All commands are
read-only except `savings record`, which appends one row to a local ledger.

## When to use axon

1. **Before heavy work** (a build, test run, Docker build, large refactor, or spawning
   subagents), check headroom:

   ```
   axon query hw_snapshot
   axon query workload_advice
   ```

   Read the `headroom` field and the workload `recommendation`. If headroom is
   `insufficient` or the recommendation is `defer`/`cooldown`/`reduce_parallelism`, tell
   the user and hold off (or reduce parallelism) instead of launching work that will OOM,
   thrash, or fail and have to be retried.

2. **When the session is slow, laggy, or a build failed unexpectedly**, find the culprit:

   ```
   axon query process_blame
   ```

   Act on the narrative -- e.g. a `gc_pressure=critical` claude process means you should
   suggest `/clear`; an oversized session file means suggest `/compact`; a runaway or
   spin-looping process should be restarted.

3. **When the user asks "how much have you saved me?"** (today / this week / this month),
   show the savings report:

   ```
   axon savings --range last_24h    # today
   axon savings --range last_7d     # this week
   axon savings --range last_30d    # this month
   ```

   Summarise the total tokens and dollars, the trend by day, and a couple of the referenced
   events so the number is backed by concrete moments.

## Logging a saving (important)

Whenever an axon signal actually changes what you do, record it so the user can see the
cumulative benefit. Pick the closest category and describe what happened:

```
axon savings record --category deferred_heavy_task \
    --detail "hw_snapshot showed headroom=insufficient (RAM 94%), so I deferred the cargo build until it recovered"
```

Categories:

- `deferred_heavy_task`    -- you deferred/reduced a build/test/heavy task under pressure
- `prevented_oom_crash`    -- you paused/freed memory before an OOM kill
- `context_reset`          -- you ran `/clear` on a GC-thrashing session
- `context_compaction`     -- you ran `/compact` on an oversized session
- `stopped_polling_loop`   -- you stopped a process re-reading a large file
- `killed_runaway_process` -- you restarted a runaway/spin-looping process
- `thermal_defer`          -- you paused work while the CPU was throttled
- `agent_cleanup`          -- you cleaned up stale/orphaned agent processes
- `disk_cleanup`           -- you cleared runaway files before the disk filled

axon fills in a conservative token estimate for the category automatically. Only pass
`--tokens N` if you have a measured figure. Do not fabricate savings -- record one only
when an axon signal genuinely changed your action.

## Notes

- axon also records preventions it detects on its own (memory/thermal/agent conditions),
  so the ledger fills in even between your explicit `record` calls.
- Everything is local. The database lives under the OS data dir (`hardware.db`); nothing is
  ever sent off-device.
