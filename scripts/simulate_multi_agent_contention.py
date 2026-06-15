#!/usr/bin/env python3
"""
Multi-agent resource contention simulation for axon.

Demonstrates the four new signals implemented based on AgentCgroup/HiveMind research:
  1. Aggregate OOM trajectory (oom_trajectory, time_to_oom_min, worst_leaking_agent_pid)
  2. Disk runaway source attribution (disk_runaway_source)
  3. Recursive log loop detection (recursive_log_loop_risk)
  4. Fine-grained ~/.claude sub-directory sizes

Usage:
    python3 scripts/simulate_multi_agent_contention.py [/path/to/axon]
"""
from __future__ import annotations

import json
import os
import subprocess
import sys
import time
from typing import Any

AXON_BIN = sys.argv[1] if len(sys.argv) > 1 else None

# ── MCP helpers ──────────────────────────────────────────────────────────────

def send(proc: subprocess.Popen, obj: dict) -> None:
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()

def read_until(proc: subprocess.Popen, req_id: int, timeout: float = 15.0) -> dict | None:
    deadline = time.time() + timeout
    while time.time() < deadline:
        line = proc.stdout.readline()
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

def mcp_start(env: dict | None = None) -> subprocess.Popen:
    """Start axon serve and complete the MCP handshake."""
    proc = subprocess.Popen(
        [axon_bin, "serve"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
        text=True, bufsize=1, env=env or os.environ.copy()
    )
    send(proc, {
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "sim", "version": "0.1.0"},
        },
    })
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    resp = read_until(proc, 0, timeout=8.0)
    if resp is None:
        raise RuntimeError("MCP handshake timed out")
    return proc

def call_tool(proc: subprocess.Popen, tool: str, params: dict | None = None,
              req_id: int = 1) -> dict | None:
    send(proc, {
        "jsonrpc": "2.0", "id": req_id, "method": "tools/call",
        "params": {"name": tool, "arguments": params or {}},
    })
    resp = read_until(proc, req_id, timeout=12.0)
    if resp is None:
        return None
    try:
        raw = resp["result"]["content"][0]["text"]
        return json.loads(raw)
    except (KeyError, TypeError, json.JSONDecodeError):
        return resp

# ── Display helpers ──────────────────────────────────────────────────────────

SEP = "=" * 70

def section(title: str) -> None:
    print(f"\n{SEP}\n  {title}\n{SEP}")

TRAJ_BADGE = {
    "safe":     "[ok]   ",
    "building": "[info] ",
    "soon":     "[warn] ",
    "imminent": "[ERR]  ",
}

def show_oom(data: dict, label: str = "") -> None:
    traj  = data.get("oom_trajectory", "safe")
    rate  = data.get("aggregate_agent_leak_rate_mb_per_hr")
    eta   = data.get("oom_time_to_impact_min") or data.get("time_to_oom_min")
    worst = data.get("worst_leaking_agent_pid") or data.get("worst_leaking_pid")
    badge = TRAJ_BADGE.get(traj, "       ")
    parts = [f"{badge}oom_trajectory={traj}"]
    if rate is not None:
        parts.append(f"agg_rate={rate:.0f} MB/hr")
    if eta is not None:
        parts.append(f"time_to_oom={eta} min")
    if worst is not None:
        parts.append(f"worst_pid={worst}")
    if label:
        parts.append(f"({label})")
    print("  " + "  ".join(parts))

def show_disk(data: dict) -> None:
    src  = data.get("disk_runaway_source")
    loop = data.get("recursive_log_loop_risk")
    dbg  = data.get("dot_claude_debug_size_gb")
    proj = data.get("dot_claude_projects_size_gb")
    tmp  = data.get("tmp_claude_tasks_size_gb")
    if src:
        print(f"  [warn] disk_runaway_source={src}")
    if loop:
        print(f"  [ERR]  recursive_log_loop_risk=true")
    if dbg is not None:
        print(f"  [info] ~/.claude/debug/    = {dbg:.3f} GB")
    if proj is not None:
        print(f"  [info] ~/.claude/projects/ = {proj:.3f} GB")
    if tmp is not None:
        print(f"  [info] /tmp/claude-*/tasks = {tmp:.3f} GB")
    if not any([src, loop, dbg, proj, tmp]):
        print("  [ok]   no disk anomalies detected")

def show_narr_oom(narrative: str) -> None:
    """Print only OOM/leak lines from a narrative string."""
    keywords = ["oom", "leak", "trajectory", "agent memory", "aggregate"]
    lines = [l.strip() for l in narrative.replace(".", ".\n").split("\n") if l.strip()]
    shown = [l for l in lines if any(k in l.lower() for k in keywords)]
    if shown:
        for l in shown:
            print(f"    > {l}")
    else:
        print("    (no OOM narrative lines — safe trajectory)")

# ── In-process logic mirror (for offline verification) ──────────────────────

def classify_disk(tasks: float, debug: float, projects: float, fill: float) -> str:
    if fill >= 0.05 and tasks > 0.5:
        return "TmpTaskOutput"
    total = debug + projects
    if total > 0.1 and debug / total > 0.4 and debug > 0.2:
        return "DebugLogLoop"
    if projects > 1.0:
        return "SessionFiles"
    return "Unknown"

def compute_traj(rates: list[float], avail_gb: float):
    PEAK = 3.0
    pos = [(r, i) for i, r in enumerate(rates) if r > 0]
    if not pos:
        return None, None, "safe", None
    agg = sum(r for r, _ in pos)
    worst_i = max(pos, key=lambda x: x[0])[1]
    rate_per_min = (agg * PEAK) / 60.0
    t = int(avail_gb * 1024 / rate_per_min)
    if t > 120:   traj = "safe"
    elif t > 30:  traj = "building"
    elif t > 10:  traj = "soon"
    else:         traj = "imminent"
    return round(agg, 1), t if traj != "safe" else None, traj, worst_i

# ── Main ─────────────────────────────────────────────────────────────────────

def main() -> None:
    global axon_bin

    repo_root = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
    debug_bin = os.path.join(repo_root, "target", "debug", "axon")

    if AXON_BIN:
        axon_bin = AXON_BIN
    elif os.path.exists(debug_bin):
        axon_bin = debug_bin
    else:
        print("[setup] building axon (debug)...")
        r = subprocess.run(["cargo", "build", "--quiet"], cwd=repo_root, capture_output=True)
        if r.returncode != 0:
            print(r.stderr.decode())
            sys.exit(1)
        axon_bin = debug_bin

    print(f"[setup] using binary: {axon_bin}")

    # ─── Scenario 1: Baseline — no Claude agents running ─────────────────────
    section("Scenario 1: Baseline — no Claude sessions running")
    proc = mcp_start()
    r = call_tool(proc, "hw_snapshot", req_id=1)
    if r and r.get("ok"):
        d = r["data"]
        show_oom(d, label="baseline — no agents")
        print(f"  [info] RAM {d.get('ram_used_gb', 0):.1f}/{d.get('ram_total_gb', 0):.0f} GB  "
              f"CPU {d.get('cpu_usage_pct', 0):.0f}%")
    else:
        print(f"  [warn] {r}")
    proc.terminate(); proc.wait()

    # ─── Scenario 2: workload_advice without OOM pressure ────────────────────
    section("Scenario 2: workload_advice for 4 subagents (clean baseline)")
    proc = mcp_start()
    r = call_tool(proc, "workload_advice",
                  {"kind": "subagents", "requested_parallelism": 4}, req_id=1)
    if r and r.get("ok"):
        d = r["data"]
        print(f"  recommendation={d.get('recommendation')}  "
              f"safe_parallelism={d.get('safe_parallelism')}  "
              f"risk={d.get('risk')}")
        show_oom(d)
    else:
        print(f"  [warn] {r}")
    proc.terminate(); proc.wait()

    # ─── Scenario 3: disk attribution — read live ~/.claude sub-dirs ──────────
    section("Scenario 3: disk attribution scan (slow-path, wait 10s for tick 4)")
    proc = mcp_start()
    print("  [wait] 10s for the slow-path sub-directory scan to fire...")
    time.sleep(10)
    r = call_tool(proc, "hw_snapshot", req_id=1)
    if r and r.get("ok"):
        d = r["data"]
        show_disk(d)
        fill = d.get("disk_fill_rate_gb_per_sec")
        print(f"  [info] disk_fill_rate_gb_per_sec={fill}  "
              f"dot_claude_size_gb={d.get('dot_claude_size_gb')}")
    else:
        print(f"  [warn] {r}")
    proc.terminate(); proc.wait()

    # ─── Scenario 4: agent_runtime_health baseline ────────────────────────────
    section("Scenario 4: agent_runtime_health — OOM fields from hw snapshot")
    proc = mcp_start()
    r = call_tool(proc, "agent_runtime_health", req_id=1)
    if r and r.get("ok"):
        d = r["data"]
        print(f"  process_count={d.get('process_count')}  "
              f"total_ram_mb={d.get('total_ram_mb', 0):.0f} MB  "
              f"total_cpu_pct={d.get('total_cpu_pct', 0):.0f}%")
        show_oom(d, label="agent_runtime_health")
    else:
        print(f"  [warn] {r}")
    proc.terminate(); proc.wait()

    # ─── Scenario 5: classify_disk_runaway_source — logic table ──────────────
    section("Scenario 5: classify_disk_runaway_source — verification table")
    cases = [
        (2.0, 0.1, 0.1, 0.39,  "TmpTaskOutput",  "#41737 — 278 GB in 12 min via tasks/"),
        (0.0, 5.0, 3.0, 0.001, "DebugLogLoop",   "#16093 — logger logs own write latencies"),
        (0.0, 0.3, 4.0, 0.001, "SessionFiles",   "projects/ dominant, no fill spike"),
        (0.0, 0.05, 0.05, 0.001, "Unknown",       "all sub-dirs too small to classify"),
        (1.0, 5.0, 3.0, 0.1,   "TmpTaskOutput",  "high fill rate wins over debug/ ratio"),
    ]
    all_ok = True
    for tasks, debug, proj, fill, expected, desc in cases:
        got = classify_disk(tasks, debug, proj, fill)
        ok = got == expected
        all_ok = all_ok and ok
        badge = "[ok]  " if ok else "[FAIL]"
        print(f"  {badge} classify({tasks:.1f}, {debug:.1f}, {proj:.1f}, {fill:.3f}) "
              f"-> {got:15s}  [{desc}]")
    print(f"\n  Result: {'all 5 pass' if all_ok else 'FAILURES detected'}")

    # ─── Scenario 6: compute_oom_trajectory — arithmetic table ───────────────
    section("Scenario 6: compute_oom_trajectory — arithmetic verification")
    traj_cases = [
        ([],            8.0, "safe",     "no agents"),
        ([-200.0],      8.0, "safe",     "only shrinking session — no OOM risk"),
        ([100.0],       0.5, "building", "100 MB/hr × 3peak = 300/hr, 512 MB free → 102 min"),
        ([200.0],       0.2, "soon",     "200 MB/hr × 3peak, 204 MB free → 20 min"),
        ([600.0, 400.0],0.4, "imminent", "1000 MB/hr × 3peak, 409 MB free → 8 min"),
    ]
    all_ok2 = True
    for rates, avail, expected, desc in traj_cases:
        agg, t, traj, worst = compute_traj(rates, avail)
        ok = traj == expected
        all_ok2 = all_ok2 and ok
        badge = "[ok]  " if ok else "[FAIL]"
        eta_str = f"{t} min" if t else "—    "
        agg_str = f"{agg:.0f} MB/hr" if agg else "0 MB/hr"
        print(f"  {badge} {traj:10s}  eta={eta_str:7s}  agg={agg_str:12s}  [{desc}]")
    print(f"\n  Result: {'all 5 pass' if all_ok2 else 'FAILURES detected'}")

    # ─── Summary ──────────────────────────────────────────────────────────────
    section("Simulation complete — signal coverage")
    print("""
  hw_snapshot new fields:
    oom_trajectory              Safe/Building/Soon/Imminent
    oom_time_to_impact_min      countdown in minutes (null when Safe)
    aggregate_agent_leak_rate_mb_per_hr  sum of positive rss_growth_rate_mb_per_hr
    worst_leaking_agent_pid     PID of the heaviest leaker
    disk_runaway_source         TmpTaskOutput/DebugLogLoop/SessionFiles/Unknown
    recursive_log_loop_risk     debug/ growing >500 MB/hr and >40% of total
    dot_claude_debug_size_gb    ~/.claude/debug/
    dot_claude_projects_size_gb ~/.claude/projects/
    tmp_claude_tasks_size_gb    /tmp/claude-{uid}/tasks/

  workload_advice new fields:
    oom_trajectory, time_to_oom_min, aggregate_agent_leak_rate_mb_per_hr
    safe_parallelism reduced by 1 when Soon; set to 0 when Imminent

  agent_runtime_health new fields:
    oom_trajectory, time_to_oom_min, aggregate_agent_leak_rate_mb_per_hr,
    worst_leaking_pid, worst_leaking_rate_mb_per_hr

  Narratives: all three tools updated with forward-looking OOM countdown text
              and disk runaway attribution commands.

  Research grounding:
    - AgentCgroup (arXiv:2602.09345): 15.4x peak-to-average memory spikes on tool calls
    - HiveMind (arXiv:2604.17111): 27% of concurrent agents died from contention
    - AIOS (arXiv:2403.16971): absence of proper scheduling in current agent designs
    - Claude GitHub issues: #41737 (278 GB /tmp), #16093 (42 GB debug loop),
      #21022 (40 MB JSONL sync hang), #31511/#33118 (node-pty ArrayBuffer leak)
""")


if __name__ == "__main__":
    main()
