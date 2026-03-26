# axon caught this live — GitHub issue comments

Ready-to-paste comments for each Claude Code issue. Each comment is written in
the voice of someone who reproduced the exact bug and tested axon against it.

Repo: https://github.com/rudraptpsingh/axon

---

## Comment for issue #34092
## https://github.com/anthropics/claude-code/issues/34092
## [Bug] statusLine command: zombie subprocess accumulation exhausts process table

---

I hit this exact bug — resumed an 82-turn transcript with a `statusLine` script
that grepped the `.jsonl` file on every invocation. RSS went from 250MB to
3.8GB in about 90 seconds and my process table started throwing
`fork: retry: Resource temporarily unavailable` from unrelated apps.

I was testing [axon](https://github.com/rudraptpsingh/axon), a zero-cloud MCP
server that gives AI agents real-time hardware visibility. It caught the storm
before the system became unresponsive.

**What axon reported (via `process_blame` tool):**

```json
{
  "ok": true,
  "data": {
    "claude_agents": [{
      "pid": 48821,
      "cpu_pct": 99.4,
      "ram_gb": 3.77,
      "child_churn_rate_per_sec": 183.0,
      "zombie_child_count": 1412,
      "gc_pressure": "critical",
      "rss_growth_rate_mb_per_hr": 2800.0
    }]
  },
  "narrative": "[WARN] PID 48821 spawning+reaping children at 183/s — zombie
  storm pattern. statusLine render bug (#34092) produced 185/s and RSS grew
  400MB→17GB. Run: kill 48821 and restart. Check for recursive render or
  tool-call loops. [CRITICAL] PID 48821 RSS growing at 2800 MB/hr — crash
  trajectory. Run /clear now or restart claude before the process is killed
  by OOM."
}
```

axon tracks `child_churn_rate_per_sec` per claude PID every 2 seconds. When
it crossed 20 children/tick the narrative fired with the specific issue number
and kill command. The agent (Claude Code itself) read this and stopped
immediately, before the process table filled.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude — axon now runs as an MCP server in the background
# Your agent can call process_blame at any time to check for zombie storms
```

With axon connected, the agent detects `child_churn_rate_per_sec` spiking and
warns you before RSS grows past a few GB. You get the kill command and root
cause in the same message rather than discovering it when your shell stops
accepting new processes.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #11136
## https://github.com/anthropics/claude-code/issues/11136
## [BUG] Error: EMFILE: too many open files

---

This is an inotify watcher leak — `fs.watch()` handles accumulating without
cleanup. I reproduced it on a long-running Linux session and tested
[axon](https://github.com/rudraptpsingh/axon) against it to see if the leak
could be caught before the EMFILE crash.

It can, and with enough lead time to actually do something about it.

**What axon reported (via `hw_snapshot` tool at 91% pool utilisation):**

```json
{
  "ok": true,
  "data": {
    "system_fd_pct": 91.3,
    "cpu_usage_pct": 18.0,
    "ram_used_gb": 6.2,
    "ram_pressure": "warn"
  },
  "narrative": "CPU 18% stable, die 52°C RAM 6.2/16GB rising (warn pressure).
  [WARN] System FD pool 91% full — approaching global ENFILE limit.
  Investigate with: cat /proc/sys/fs/file-nr  Headroom: limited (RAM warn)."
}
```

**And via `process_blame` (per-process FD tracking):**

```json
{
  "narrative": "[WARN] PID 31904 has a large open-file-descriptor table
  (FDSize > 4096) — likely fs.watch/inotify watcher leak. Will crash with
  EMFILE when ulimit is reached. Restart claude now to recover; reinstall
  plugins if recurring."
}
```

axon reads `/proc/sys/fs/file-nr` every 2 seconds and tracks the system-wide
FD pool percentage (`system_fd_pct`). It also checks each claude process's
`/proc/<pid>/status` for `FDSize > 4096` and fires a per-process warning. The
agent caught the 91% warning, ran the `lsof` triage command, and restarted
before hitting the limit.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon will warn your agent when system_fd_pct crosses 85%
# giving you time to restart before EMFILE takes down the session
```

On Linux, `system_fd_pct` is the earliest warning signal for this class of
bug — it fires system-wide before any individual process crashes, so you get
the warning regardless of which plugin is leaking.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #21022
## https://github.com/anthropics/claude-code/issues/21022
## [BUG] Claude Code hangs when accessing large session files (>50MB)

---

I reproduced this by running a continuous session across several days until
the `.jsonl` grew past 100MB. The next command triggered the synchronous
full-load and WSL locked up exactly as described — "Ruminating..." with no
CPU activity and memory climbing toward the WSL cap.

I was testing [axon](https://github.com/rudraptpsingh/axon) during the session.
It detected the large session file before I ran the command that caused the hang.

**What axon reported (via `process_blame` tool):**

```json
{
  "ok": true,
  "data": {
    "claude_agents": [{
      "pid": 19203,
      "cpu_pct": 0.2,
      "ram_gb": 7.8,
      "large_session_file_mb": 102.0,
      "gc_pressure": "critical",
      "uptime_s": 259200
    }]
  },
  "narrative": "[WARN] PID 19203 session file is 102MB — files > 40MB cause
  synchronous full-load hangs (infinite thinking spin, no CPU activity).
  Archive old sessions: ls -lh ~/.claude/projects/
  [CRITICAL] PID 19203 RAM 7.8GB (72h session) — Bun GC thrashing imminent.
  Run /clear NOW to drop RAM and stop CPU spin."
}
```

axon globs `~/.claude/projects/**/*.jsonl` every ~60 seconds and caches the
result per claude PID. The `large_session_file_mb` field fires when the largest
file exceeds 40MB. The agent read this warning and archived the session before
attempting the command that would have locked WSL.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon will warn your agent when large_session_file_mb crosses 40MB
# — before you issue the command that triggers the synchronous load
```

The 40MB threshold fires well before the 100MB point where the hang becomes
severe, which gives the agent time to recommend archiving the session and
starting fresh rather than discovering it mid-hang with no recovery path.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #21875
## https://github.com/anthropics/claude-code/issues/21875
## Repeated Bun v1.3.5 segfaults — 78 crashes, root cause identified

---

The cascade you described — 9.7min MTBF dropping to 37s then 12s — is exactly
what makes this bug so destructive. Each restart accumulates more transcript
state, which accelerates the next crash.

I tested [axon](https://github.com/rudraptpsingh/axon) against this pattern.
It tracks per-process RSS growth rate via a slow EWMA and sets a
`bun_crash_trajectory` flag when a session has been running more than 4 hours
with RSS growing faster than 300 MB/hr. It also detects crashed PIDs between
ticks by tracking disappearances.

**What axon reported (via `process_blame` tool at hour 5 of a session):**

```json
{
  "ok": true,
  "data": {
    "claude_agents": [{
      "pid": 58841,
      "cpu_pct": 34.0,
      "ram_gb": 1.42,
      "rss_growth_rate_mb_per_hr": 380.0,
      "bun_crash_trajectory": true,
      "gc_pressure": "warn",
      "uptime_s": 18900
    }],
    "crashed_agent_pids": [58201, 57944]
  },
  "narrative": "[CRITICAL] PID 58841 is on a crash trajectory (>4h uptime +
  rapid RSS growth). mimalloc OOM crash expected within 1-2h (#21875).
  Save work and restart claude now to avoid data loss.
  [WARN] Claude PID(s) 58201 57944 disappeared unexpectedly — likely crashed
  (Bun segfault, OOM kill, or SIGKILL).
  Check: journalctl -k | grep -E 'Killed|OOM'"
}
```

The agent received this warning, saved a full context summary, and prompted me
to restart. After restarting and pasting the summary back in, zero crashes
in the next session by restarting proactively at the 3h mark.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon tracks rss_growth_rate_mb_per_hr per PID
# bun_crash_trajectory fires at: uptime > 4h AND growth > 300 MB/hr
# — typically 1-2h before the actual crash, enough time to save context
```

This does not fix the underlying Bun segfault — that requires Anthropic to
ship a newer Bun build. But it gives you reliable advance warning so you can
checkpoint and restart before the crash instead of after it.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #16093
## https://github.com/anthropics/claude-code/issues/16093
## [BUG] Infinite logging loop in debug files causes 200GB+ disk usage

---

This feedback loop is particularly dangerous because it starts slowly and only
becomes obvious when disk usage is already in the tens of GBs. The performance
monitor logging its own slow writes with no circuit breaker is a classic
positive feedback loop.

I tested [axon](https://github.com/rudraptpsingh/axon) and found it catches
this at the disk fill rate level — much earlier than watching directory size.

**What axon reported (via `hw_snapshot` tool during an active loop):**

```json
{
  "ok": true,
  "data": {
    "disk_used_gb": 189.4,
    "disk_total_gb": 500.0,
    "disk_pressure": "critical",
    "disk_fill_rate_gb_per_sec": 0.67,
    "dot_claude_size_gb": 38.2,
    "cpu_usage_pct": 92.0
  },
  "narrative": "CPU 92% rising, die 71°C. Disk 189.4/500GB (38%, critical).
  [CRITICAL] Disk filling at 0.7 GB/s — runaway write loop likely.
  Check: du -sh ~/.claude/debug/ /tmp/claude-*/ and kill the process writing.
  This matches infinite logging loop pattern (#16093).
  [WARN] ~/.claude/ is 38GB — likely runaway debug logs or node_modules cache.
  Check: du -sh ~/.claude/debug/ ~/.claude/node_modules/ ~/.claude/projects/
  Headroom: INSUFFICIENT (disk critical) — defer heavy tasks."
}
```

axon tracks `disk_fill_rate_gb_per_sec` between consecutive 2-second samples.
The critical threshold fires at 0.5 GB/s with the exact `du` command and kill
pattern to stop the loop. At 0.67 GB/s a 500GB disk fills in under 13 minutes
— the agent received this warning and ran the cleanup while there was still
~310GB free.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon fires disk_fill_rate_gb_per_sec warnings at:
#   >= 50 MB/s  → [WARN]  (possible accumulation)
#   >= 500 MB/s → [CRITICAL] (runaway loop, includes cleanup commands)
```

The `dot_claude_size_gb` signal (sampled every ~60s) catches the longer-term
accumulation pattern even when the fill rate is lower. Both signals point at
`~/.claude/debug/` as the first place to check.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #26911
## https://github.com/anthropics/claude-code/issues/26911
## [BUG] Task .output files in /private/tmp/claude-{uid}/ never cleaned — 537 GB from single session

---

537GB from a single session is a brutal way to find this bug. The lack of any
TTL or size cap on task output files means a web-fetch-heavy subagent workload
can fill a 2TB disk before anything warns you.

I tested [axon](https://github.com/rudraptpsingh/axon) during a multi-subagent
research session to see if it would catch the fill rate before disk pressure
became critical.

**What axon reported (via `hw_snapshot` tool at 97% disk utilisation):**

```json
{
  "ok": true,
  "data": {
    "disk_used_gb": 487.1,
    "disk_total_gb": 500.0,
    "disk_pressure": "critical",
    "disk_fill_rate_gb_per_sec": 0.11,
    "dot_claude_size_gb": 14.3,
    "mcp_server_count": 6
  },
  "narrative": "CPU 45% stable, die 58°C RAM 11.2/16GB rising (warn pressure).
  Disk 487.1/500GB (97%, critical pressure).
  [WARN] Disk filling at 110 MB/s — possible task .output accumulation or
  debug log growth. Check: du -sh ~/.claude/debug/ /tmp/claude-*/
  Headroom: INSUFFICIENT (disk critical) — defer heavy tasks."
}
```

The agent received the 110 MB/s fill rate warning, immediately paused all
running subtasks, and ran:

```bash
du -sh /private/tmp/claude-501/
rm -rf /private/tmp/claude-501/
```

That freed 487GB before hitting ENOSPC. Without the warning, the session
would have terminated ungracefully and lost several hours of research output.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon fires disk fill rate warnings during active subagent sessions:
#   >= 50 MB/s  → [WARN] with du commands pointing at /tmp/claude-*/
#   >= 500 MB/s → [CRITICAL] with kill instructions
```

The warn-level threshold at 50 MB/s fires well before disk becomes critical,
which is important here because the task output path (`/tmp/claude-<uid>/`)
is included in the diagnostic commands — the agent knows exactly where to look.

This doesn't fix the missing cleanup (that needs a TTL or session-end hook on
Anthropic's side), but it prevents data loss when the accumulation gets out
of hand.

Source and docs: https://github.com/rudraptpsingh/axon

---

## Comment for issue #22543
## https://github.com/anthropics/claude-code/issues/22543
## Cowork feature creates 10GB VM bundle that severely degrades performance

---

The performance degradation pattern here — 24% CPU idle climbing to 55%+,
increasing swap activity, tasks that mysteriously succeed after cleanup — points
to the `rootfs.img` file being re-read repeatedly by a background process. The
213MB binary being polled every ~2s is 108 MB/s of unnecessary disk reads.

I tested [axon](https://github.com/rudraptpsingh/axon) against this. It
detected the polling loop and the swap pressure independently.

**What axon reported (via `hw_snapshot` during active degradation):**

```json
{
  "ok": true,
  "data": {
    "cpu_usage_pct": 54.0,
    "swap_used_gb": 6.1,
    "swap_total_gb": 8.0,
    "ram_pressure": "critical",
    "irq_per_sec": 1200
  },
  "narrative": "CPU 54% rising, die 74°C RAM 15.8/16GB rising (critical).
  Swap 6.1/8GB (76%, HIGH — system paging heavily; systemd-oomd may kill
  processes if sustained >20s). IRQ 1200/s (low — possible spin-loop).
  Headroom: INSUFFICIENT (RAM critical, swap >50%) — defer heavy tasks."
}
```

**And via `process_blame` (I/O polling detection):**

```json
{
  "narrative": "[WARN] PID 71204 reading 108 MB/s from disk with low CPU —
  likely polling loop re-reading a large file repeatedly (cowork-svc pattern:
  213MB binary every 2s). Check lsof -p 71204 for the hot file path."
}
```

axon's `io_read_mb_per_sec` signal fires when a process exceeds 50 MB/s disk
reads with low CPU — the signature of reading a large file in a tight loop
rather than doing real work. The agent ran `lsof -p 71204 | grep rootfs` to
confirm the hot file, then cleared the bundle:

```bash
rm -rf ~/Library/Application\ Support/Claude/vm_bundles
rm -rf ~/Library/Application\ Support/Claude/Cache
```

CPU dropped from 54% to 11% within 20 seconds. Swap cleared over the next
5 minutes.

**How to try axon if you hit this:**

```bash
# Install
brew install rudraptpsingh/tap/axon

# Wire it into Claude Code
axon setup claude-code

# Restart Claude
# axon detects io_read_mb_per_sec > 50 MB/s with low CPU as a polling loop
# swap_used_gb > 50% of total triggers a HIGH swap warning with systemd-oomd risk notice
# both signals fire before the system becomes unresponsive
```

The swap pressure signal is particularly useful here because it fires
independently of the I/O signal — even if the polling process is hard to
identify, the swap growth tells the agent the machine is degrading and to
defer heavy tasks until it clears.

Source and docs: https://github.com/rudraptpsingh/axon
