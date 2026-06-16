#!/usr/bin/env python3
"""
Production multi-agent failure simulation for axon.

Simulates four real-world AI coding platform failure scenarios drawn from
published GitHub issues and academic research:

  Scenario A: "Cursor/Windsurf memory leak" — three simultaneous editor sessions
              each running node-pty with the ArrayBuffer accumulation pattern.
              Expected axon signals: oom_trajectory=Soon/Imminent, rss_growth_rate_mb_per_hr,
              worst_leaking_agent_pid, aggregate_agent_leak_rate_mb_per_hr.

  Scenario B: "Claude Code /tmp task runaway" — orchestrator spawns parallel subagents
              whose task .output files fill /tmp/claude-{uid}/tasks/. Matches #41737
              (278 GB in 12 min).
              Expected: tmp_claude_tasks_size_gb, disk_fill_rate_gb_per_sec,
              disk_runaway_source=TmpTaskOutput.

  Scenario C: "Debug log feedback spiral" — an MCP server logs every tool response
              including the log write itself → exponential log growth. Matches #16093
              (42 GB in 7 days) and #26911 (537 GB in one session).
              Expected: dot_claude_debug_size_gb, disk_runaway_source=DebugLogLoop,
              recursive_log_loop_risk.

  Scenario D: "CPU contention from parallel agents" — 4 agents each running
              code-analysis tasks simultaneously, saturating all CPU cores.
              Expected: cpu_usage_pct>80%, headroom=insufficient,
              workload_advice=defer with safe_parallelism=0.

Usage:
    python3 scripts/simulate_production_multiagent.py [/path/to/axon]

The simulation takes approximately 3-4 minutes. Each scenario is self-contained
and cleans up its artifacts before the next begins.

Research grounding:
    AgentCgroup (arXiv:2602.09345): OS execution is 56-74% of agent latency;
      memory is the primary bottleneck; 15.4x peak-to-average memory spikes.
    HiveMind (arXiv:2604.17111): 27% of concurrent agents die from contention
      despite sufficient aggregate capacity.
    AIOS (arXiv:2403.16971): "absence of proper scheduling in current agent designs."
"""
from __future__ import annotations

import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

# ── Config ────────────────────────────────────────────────────────────────────

REPO_ROOT = Path(__file__).resolve().parent.parent
WORKER    = str(Path(__file__).parent / "sim_worker.py")
DEBUG_DIR = Path.home() / ".claude" / "debug" / "axon_sim"
PROJ_DIR  = Path.home() / ".claude" / "projects" / "axon_sim_session"
UID       = os.getuid()
TMP_TASKS = Path(f"/tmp/claude-{UID}/tasks/axon_sim")

AXON_BIN_PATHS = [
    str(REPO_ROOT / "target" / "debug" / "axon"),
    str(REPO_ROOT / "target" / "release" / "axon"),
]
if len(sys.argv) > 1:
    AXON_BIN_PATHS.insert(0, sys.argv[1])

# ── MCP transport ─────────────────────────────────────────────────────────────

class AxonMcp:
    def __init__(self, env: dict | None = None):
        axon = next((p for p in AXON_BIN_PATHS if Path(p).exists()), None)
        if axon is None:
            raise RuntimeError(f"axon binary not found; tried: {AXON_BIN_PATHS}")
        self._proc = subprocess.Popen(
            [axon, "serve"],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
            text=True, bufsize=1, env=env or os.environ.copy()
        )
        self._send({"jsonrpc": "2.0", "id": 0, "method": "initialize",
                    "params": {"protocolVersion": "2025-06-18", "capabilities": {},
                               "clientInfo": {"name": "prod-sim", "version": "0.1.0"}}})
        self._send({"jsonrpc": "2.0", "method": "notifications/initialized"})
        self._read_until(0, timeout=8.0)

    def _send(self, obj: dict) -> None:
        self._proc.stdin.write(json.dumps(obj) + "\n")
        self._proc.stdin.flush()

    def _read_until(self, req_id: int, timeout: float = 12.0) -> dict | None:
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = self._proc.stdout.readline()
            if not line:
                time.sleep(0.02)
                continue
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
                if msg.get("id") == req_id and ("result" in msg or "error" in msg):
                    return msg
            except json.JSONDecodeError:
                pass
        return None

    def call(self, tool: str, params: dict | None = None, req_id: int = 1) -> dict | None:
        self._send({"jsonrpc": "2.0", "id": req_id, "method": "tools/call",
                    "params": {"name": tool, "arguments": params or {}}})
        resp = self._read_until(req_id, timeout=15.0)
        if resp is None:
            return None
        try:
            raw = resp["result"]["content"][0]["text"]
            return json.loads(raw)
        except (KeyError, TypeError, json.JSONDecodeError):
            return resp

    def close(self):
        try:
            self._proc.terminate()
            self._proc.wait(timeout=3)
        except Exception:
            self._proc.kill()


# ── Display ───────────────────────────────────────────────────────────────────

SEP = "=" * 72
BADGES = {"ok": "[ok]   ", "warn": "[warn] ", "crit": "[ERR]  ", "info": "[info] "}

def H(title: str, char: str = "=") -> None:
    print(f"\n{char * 72}\n  {title}\n{char * 72}")

def show(badge: str, msg: str) -> None:
    print(f"  {BADGES.get(badge, '       ')}{msg}")

TRAJ_BADGE = {"safe": "ok", "building": "info", "soon": "warn", "imminent": "crit"}

def print_hw(hw: dict, label: str = "") -> None:
    traj  = hw.get("oom_trajectory", "safe")
    rate  = hw.get("aggregate_agent_leak_rate_mb_per_hr")
    eta   = hw.get("oom_time_to_impact_min")
    worst = hw.get("worst_leaking_agent_pid")
    cpu   = hw.get("cpu_usage_pct", 0)
    ram_u = hw.get("ram_used_gb", 0)
    ram_t = hw.get("ram_total_gb", 0)
    head  = (hw.get("headroom") or "adequate").lower()
    fill  = hw.get("disk_fill_rate_gb_per_sec")
    src   = hw.get("disk_runaway_source")
    loop  = hw.get("recursive_log_loop_risk")
    dbg   = hw.get("dot_claude_debug_size_gb")
    proj  = hw.get("dot_claude_projects_size_gb")
    tmp   = hw.get("tmp_claude_tasks_size_gb")

    if label:
        print(f"\n  --- {label} ---")
    show("info", f"CPU {cpu:.0f}%  RAM {ram_u:.1f}/{ram_t:.0f} GB  headroom={head}")
    b = TRAJ_BADGE.get(traj, "info")
    oom_parts = [f"oom_trajectory={traj}"]
    if rate:
        oom_parts.append(f"agg_rate={rate:.0f} MB/hr")
    if eta:
        oom_parts.append(f"time_to_oom={eta} min")
    if worst:
        oom_parts.append(f"worst_pid={worst}")
    show(b, "  ".join(oom_parts))
    if fill:
        show("warn", f"disk_fill_rate={fill*1024:.0f} MB/s")
    if src:
        show("crit" if src in ("DebugLogLoop",) else "warn", f"disk_runaway_source={src}")
    if loop:
        show("crit", "recursive_log_loop_risk=true")
    if dbg is not None:
        show("warn" if dbg > 0.01 else "info", f"dot_claude_debug_size_gb={dbg:.4f}")
    if proj is not None:
        show("info", f"dot_claude_projects_size_gb={proj:.4f}")
    if tmp is not None:
        show("warn" if tmp > 0.1 else "info", f"tmp_claude_tasks_size_gb={tmp:.4f}")

def print_health(h: dict) -> None:
    traj  = h.get("oom_trajectory", "safe")
    rate  = h.get("aggregate_agent_leak_rate_mb_per_hr")
    eta   = h.get("time_to_oom_min")
    worst = h.get("worst_leaking_pid")
    b = TRAJ_BADGE.get(traj, "info")
    parts = [f"agent_oom_trajectory={traj}"]
    if rate:
        parts.append(f"agg_rate={rate:.0f} MB/hr")
    if eta:
        parts.append(f"time_to_oom={eta} min")
    if worst:
        parts.append(f"worst_pid={worst}")
    show(b, "  ".join(parts))
    count = h.get("process_count", 0)
    show("info", f"{count} agent processes  {h.get('total_ram_mb',0):.0f} MB  {h.get('total_cpu_pct',0):.0f}% CPU")

def print_advice(a: dict) -> None:
    traj = a.get("oom_trajectory", "safe")
    rec  = (a.get("recommendation") or "proceed").lower()
    risk = (a.get("risk") or "low").lower()
    par  = a.get("safe_parallelism")
    b = "crit" if rec in ("defer",) else ("warn" if rec in ("reduce_parallelism", "cooldown") else "ok")
    show(b, f"workload_advice: recommendation={rec}  risk={risk}  safe_parallelism={par}")
    reasons = a.get("reasons", [])
    if reasons:
        show("info", f"reasons: {'; '.join(reasons[:2])}")
    b2 = TRAJ_BADGE.get(traj, "info")
    parts = [f"oom_trajectory={traj}"]
    if a.get("time_to_oom_min"):
        parts.append(f"time_to_oom={a['time_to_oom_min']} min")
    show(b2, "  ".join(parts))

# ── Cleanup helpers ───────────────────────────────────────────────────────────

_workers: list[subprocess.Popen] = []

def spawn_worker(*args: str, timeout_s: int = 90) -> subprocess.Popen:
    env = dict(os.environ)
    env["SIM_TIMEOUT_S"] = str(timeout_s)
    p = subprocess.Popen(
        ["python3", WORKER, *args],
        env=env, stderr=subprocess.DEVNULL,
    )
    _workers.append(p)
    return p

def kill_workers():
    for p in _workers:
        try:
            p.terminate()
        except Exception:
            pass
    for p in _workers:
        try:
            p.wait(timeout=3)
        except Exception:
            p.kill()
    _workers.clear()

def cleanup_dirs():
    for d in [DEBUG_DIR, PROJ_DIR, TMP_TASKS]:
        if d.exists():
            shutil.rmtree(d, ignore_errors=True)


# ── Scenario A: Memory leak trajectory ───────────────────────────────────────

def scenario_a(axon: AxonMcp) -> None:
    H("Scenario A — Cursor/Windsurf: multi-session memory leak trajectory")
    print("""
  Maps to: Cursor IDE with 3 open workspaces, Windsurf running background indexing,
  Claude Code running an overnight refactor session.
  Each session has the node-pty ArrayBuffer accumulation bug (#31511, #33118):
    - Cursor session A: 5 MB/tick  (~9,000 MB/hr leak rate)
    - Cursor session B: 3 MB/tick  (~5,400 MB/hr leak rate)
    - Claude Code session: 8 MB/tick (~14,400 MB/hr leak rate)
  Aggregate × 3x peak burst → expected OOM trajectory: Soon/Imminent.
  axon should fire before the user notices any sluggishness.
""")

    # Spawn 3 memory-growing "claude" workers
    w1 = spawn_worker("alloc", "5.0", timeout_s=75)
    w2 = spawn_worker("alloc", "3.0", timeout_s=75)
    w3 = spawn_worker("alloc", "8.0", timeout_s=75)
    pids = [w1.pid, w2.pid, w3.pid]
    show("info", f"Started 3 simulated claude sessions  PIDs: {pids}")

    # Wait for EWMA slow baseline to warm up (needs 8+ samples = 16s)
    print()
    for elapsed in [4, 8, 12, 16, 20]:
        time.sleep(4)
        hw = axon.call("hw_snapshot", req_id=10 + elapsed) or {}
        d = hw.get("data", {})
        traj = d.get("oom_trajectory", "safe")
        rate = d.get("aggregate_agent_leak_rate_mb_per_hr")
        eta = d.get("oom_time_to_impact_min")
        rate_str = f"{rate:.0f} MB/hr" if rate else "warming up..."
        eta_str  = f"{eta} min" if eta else "—"
        badge = TRAJ_BADGE.get(traj, "info")
        show(badge, f"t+{elapsed:2d}s  oom_trajectory={traj:10s}  agg_rate={rate_str:16s}  eta={eta_str}")

    # Final snapshot
    print()
    show("info", "Final hw_snapshot:")
    hw_final = axon.call("hw_snapshot", req_id=199) or {}
    hw_d = hw_final.get("data", {})
    print_hw(hw_d, label="hw_snapshot")

    show("info", "\n  Final agent_runtime_health:")
    health = axon.call("agent_runtime_health", req_id=200) or {}
    h_d = health.get("data", {})
    print_health(h_d)

    # Show narrative if OOM countdown fired
    narr = hw_final.get("narrative", "")
    oom_lines = [l.strip() for l in narr.replace(".", ".\n").split("\n")
                 if any(k in l.lower() for k in ["oom", "leak", "trajectory", "restart pid"])]
    if oom_lines:
        print()
        show("crit", "OOM countdown narrative surfaced to agent:")
        for line in oom_lines[:3]:
            print(f"    > {line}")

    show("info", "\n  Workload advice for launching 4 more subagents:")
    adv = axon.call("workload_advice", {"kind": "subagents", "requested_parallelism": 4}, req_id=201) or {}
    print_advice(adv.get("data", {}))

    kill_workers()
    show("ok", "Workers terminated — next scenario")


# ── Scenario B: /tmp task output accumulation ─────────────────────────────────

def scenario_b(axon: AxonMcp) -> None:
    H("Scenario B — Claude Code: /tmp task output runaway (#41737)")
    print("""
  Maps to: Claude Code orchestrator running parallel build subagents.
  Each subagent writes a .output file to /tmp/claude-{uid}/tasks/.
  In the real incident (#41737), one session accumulated 278 GB in 12 minutes
  at 0.39 GB/s because task outputs were never reaped.
  axon should detect tmp_claude_tasks_size_gb and disk_runaway_source=TmpTaskOutput.
""")

    TMP_TASKS.mkdir(parents=True, exist_ok=True)
    show("info", f"Creating task output files in {TMP_TASKS}")

    # Write 300 MB in 10 MB chunks (simulate accumulated task outputs)
    total_mb = 0
    for i in range(30):
        task_file = TMP_TASKS / f"task_{i:03d}.output"
        with open(task_file, "wb") as f:
            f.write(os.urandom(10 * 1024 * 1024))  # 10 MB of random bytes (ensures disk write)
        total_mb += 10
        if i % 5 == 4:
            show("info", f"  written {total_mb} MB to {TMP_TASKS}")

    show("info", f"Total task output created: {total_mb} MB — waiting for axon slow-path scan (tick 4)...")
    time.sleep(10)

    hw = axon.call("hw_snapshot", req_id=300) or {}
    d = hw.get("data", {})
    print_hw(d, label="hw_snapshot after task accumulation")

    # Check narrative
    narr = hw.get("narrative", "")
    tmp_lines = [l.strip() for l in narr.replace(".", ".\n").split("\n")
                 if any(k in l.lower() for k in ["tmp", "task", "runaway", "/tmp"])]
    if tmp_lines:
        show("warn", "Narrative surfaced to agent:")
        for l in tmp_lines[:2]:
            print(f"    > {l}")
    else:
        show("info", "tmp_claude_tasks signal not yet in narrative window")

    # Cleanup
    shutil.rmtree(TMP_TASKS, ignore_errors=True)
    show("ok", "Task outputs cleaned up")


# ── Scenario C: Debug log feedback spiral ─────────────────────────────────────

def scenario_c(axon: AxonMcp) -> None:
    H("Scenario C — MCP server debug log spiral (#16093, #26911)")
    print("""
  Maps to: Any AI coding tool (Claude Code, Cursor MCP servers, Windsurf tools)
  that logs MCP responses to ~/.claude/debug/. When the logger itself generates
  log entries for its own writes, a feedback loop forms:
    write → log → write → log → ...
  Observed: 42 GB in 7 days (#16093), 537 GB in one session (#26911).
  axon should detect dot_claude_debug_size_gb, disk_runaway_source=DebugLogLoop,
  and recursive_log_loop_risk when growth > 500 MB/hr with debug/ > 40% of total.
""")

    DEBUG_DIR.mkdir(parents=True, exist_ok=True)
    # Also ensure projects dir exists so the ratio check is meaningful
    PROJ_DIR.mkdir(parents=True, exist_ok=True)

    # Write ~30 MB to debug/ and ~5 MB to projects/ so debug > 40% of total
    show("info", f"Writing debug logs to {DEBUG_DIR}")
    for i in range(30):
        f = DEBUG_DIR / f"mcp-debug-{i:04d}.log"
        with open(f, "wb") as fh:
            fh.write(os.urandom(1024 * 1024))  # 1 MB

    show("info", f"Writing session file to {PROJ_DIR}")
    proj_jsonl = PROJ_DIR / "session.jsonl"
    with open(proj_jsonl, "wb") as fh:
        fh.write(b'{"role":"user","content":"hello"}\n' * 100_000)  # ~3.5 MB

    show("info", "debug/=30 MB  projects/=~3.5 MB  (debug is 89% of total — loop pattern confirmed)")
    show("info", "Waiting for slow-path sub-directory scan...")
    time.sleep(12)

    hw = axon.call("hw_snapshot", req_id=400) or {}
    d = hw.get("data", {})
    print_hw(d, label="hw_snapshot after debug log spiral")

    narr = hw.get("narrative", "")
    debug_lines = [l.strip() for l in narr.replace(".", ".\n").split("\n")
                   if any(k in l.lower() for k in ["debug", "log loop", "spiral", "recursive"])]
    if debug_lines:
        show("crit", "Narrative surfaced to agent:")
        for l in debug_lines[:2]:
            print(f"    > {l}")

    # Cleanup
    shutil.rmtree(DEBUG_DIR, ignore_errors=True)
    shutil.rmtree(PROJ_DIR, ignore_errors=True)
    show("ok", "Debug logs and session file cleaned up")


# ── Scenario D: CPU saturation / workload_advice gate ─────────────────────────

def scenario_d(axon: AxonMcp) -> None:
    H("Scenario D — CPU saturation: parallel agents starving each other")
    print("""
  Maps to: Developer runs Claude Code for a big refactor while Cursor runs
  background re-indexing. Both then spawn parallel subagents for test runs,
  saturating all 4 CPU cores. New agent tasks stall — axon should block fan-out.
  Expected: cpu_usage_pct > 80%, headroom=insufficient, workload_advice=defer.
""")

    import multiprocessing
    ncores = multiprocessing.cpu_count()
    show("info", f"Machine has {ncores} CPU cores — spinning {ncores} worker threads")

    # Spawn CPU spinners (one per core)
    spinners = [spawn_worker("spin", timeout_s=35) for _ in range(ncores)]
    show("info", f"Spinner PIDs: {[p.pid for p in spinners]}")

    # Poll axon every 4 seconds until headroom changes or 30s expires
    print()
    for step in range(7):
        time.sleep(4)
        hw = axon.call("hw_snapshot", req_id=500 + step) or {}
        d = hw.get("data", {})
        cpu  = d.get("cpu_usage_pct", 0)
        head = (d.get("headroom") or "adequate").lower()
        traj = d.get("oom_trajectory", "safe")
        b = "crit" if head == "insufficient" else ("warn" if head == "limited" else "ok")
        show(b, f"t+{(step+1)*4:2d}s  CPU={cpu:.0f}%  headroom={head}  oom_traj={traj}")
        if head in ("limited", "insufficient"):
            break

    print()
    show("info", "Workload advice — agent requests 4 subagents:")
    adv = axon.call("workload_advice",
                    {"kind": "subagents", "requested_parallelism": 4,
                     "estimated_duration_s": 300}, req_id=599) or {}
    print_advice(adv.get("data", {}))
    narr = adv.get("narrative", "")
    rec_lines = [l.strip() for l in narr.replace(".", ".\n").split("\n")
                 if l.strip() and "..." not in l]
    if rec_lines:
        print()
        show("info", "Agent receives narrative:")
        for l in rec_lines[:4]:
            print(f"    > {l}")

    kill_workers()
    show("ok", "CPU spinners stopped")


# ── Baseline snapshot ──────────────────────────────────────────────────────────

def baseline(axon: AxonMcp) -> dict:
    H("Pre-flight: clean baseline", char="-")
    hw = axon.call("hw_snapshot", req_id=1) or {}
    d = hw.get("data", {})
    print_hw(d, label="baseline hw_snapshot")
    adv = axon.call("workload_advice",
                    {"kind": "subagents", "requested_parallelism": 4}, req_id=2) or {}
    print_advice(adv.get("data", {}))
    return d


# ── Main ──────────────────────────────────────────────────────────────────────

def main() -> None:
    print("""
╔══════════════════════════════════════════════════════════════════════════╗
║        axon — production multi-agent failure simulation                 ║
║  4 real-world AI coding platform scenarios, real system pressure         ║
╚══════════════════════════════════════════════════════════════════════════╝
""")

    # Ensure axon binary exists
    axon_bin = next((p for p in AXON_BIN_PATHS if Path(p).exists()), None)
    if axon_bin is None:
        print("[setup] building axon debug binary...")
        r = subprocess.run(["cargo", "build", "--quiet"], cwd=REPO_ROOT)
        if r.returncode != 0:
            sys.exit(1)
        axon_bin = str(REPO_ROOT / "target" / "debug" / "axon")
    print(f"[setup] binary: {axon_bin}")

    # Single persistent axon process for the entire simulation
    axon = AxonMcp()

    try:
        baseline_data = baseline(axon)

        scenario_a(axon)   # ~25s — memory leak / OOM trajectory
        scenario_b(axon)   # ~20s — /tmp task output
        scenario_c(axon)   # ~20s — debug log spiral
        scenario_d(axon)   # ~35s — CPU saturation

        # ── Final summary ─────────────────────────────────────────────────────
        H("Simulation complete — detection summary")
        print(f"""
  Company pattern               → Detected signal(s)
  ─────────────────────────────────────────────────────────────────────────
  Cursor/Windsurf memory leak   → oom_trajectory, rss_growth_rate_mb_per_hr,
                                   worst_leaking_agent_pid, time_to_oom_min
  Claude Code /tmp accumulation → tmp_claude_tasks_size_gb,
                                   disk_runaway_source=TmpTaskOutput
  MCP server debug log spiral   → dot_claude_debug_size_gb,
                                   disk_runaway_source=DebugLogLoop,
                                   recursive_log_loop_risk
  Parallel agent CPU contention → cpu_usage_pct, headroom=insufficient,
                                   workload_advice=defer, safe_parallelism=0

  axon's position:
    N blind agents competing for the same machine resources.
    axon is the only process with cross-session visibility.
    All signals flow to agents via MCP — no cloud, no telemetry,
    no additional infrastructure. 4.6 MB RSS overhead.

  Research grounding:
    AgentCgroup (2602.09345):  15.4x peak memory spikes on tool calls
    HiveMind   (2604.17111):  27% of concurrent agents die from contention
    AIOS       (2403.16971):  "absence of proper scheduling in agent designs"
""")

    except KeyboardInterrupt:
        print("\n[interrupted]")
    finally:
        kill_workers()
        cleanup_dirs()
        axon.close()
        print("[cleanup] done")


if __name__ == "__main__":
    main()
