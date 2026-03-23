#!/usr/bin/env python3
"""
CPU Stress Evaluation for Axon Agent.

Phases:
  1. Baseline      – 12s quiescent, take hw_snapshot + process_blame
  2. CPU Stress    – spawn N `yes > /dev/null` workers, observe for 30s
  3. Peak Capture  – take hw_snapshot + process_blame at peak load
  4. Cooldown      – kill stress, wait 12s, capture recovery state
  5. Session Health – query session_health for the entire window
  6. Hardware Trend – query hardware_trend for last_1h
  7. GPU Snapshot   – query gpu_snapshot
  8. Serve + Alert  – verify serve lifecycle and alert triggers

Usage: python3 scripts/eval_cpu_stress.py ./target/release/axon
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from typing import Any

AXON = sys.argv[1] if len(sys.argv) > 1 else "./target/release/axon"
NUM_STRESS_WORKERS = 4  # Match core count
MSG_ID = 0


def next_id() -> int:
    global MSG_ID
    MSG_ID += 1
    return MSG_ID


def send(proc: subprocess.Popen, obj: dict[str, Any]) -> None:
    assert proc.stdin
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def read_responses(proc: subprocess.Popen, until_id: int, timeout_s: float = 45.0) -> dict[str, Any]:
    buf: list[dict[str, Any]] = []
    deadline = time.time() + timeout_s
    assert proc.stdout
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        buf.append(msg)
        if msg.get("id") == until_id and "result" in msg:
            return msg
    raise RuntimeError(f"timeout waiting for id={until_id}; last msgs: {buf[-3:]!r}")


def call_tool(proc: subprocess.Popen, name: str, args: dict | None = None) -> dict[str, Any]:
    mid = next_id()
    send(proc, {
        "jsonrpc": "2.0",
        "id": mid,
        "method": "tools/call",
        "params": {"name": name, "arguments": args or {}},
    })
    res = read_responses(proc, mid)
    content = res["result"]["content"]
    text = content[0]["text"]
    return json.loads(text)


def list_tools(proc: subprocess.Popen) -> list[str]:
    mid = next_id()
    send(proc, {"jsonrpc": "2.0", "id": mid, "method": "tools/list", "params": {}})
    res = read_responses(proc, mid)
    tools = res.get("result", {}).get("tools", [])
    return [t["name"] for t in tools]


def initialize_mcp(proc: subprocess.Popen) -> None:
    mid = next_id()
    send(proc, {
        "jsonrpc": "2.0",
        "id": mid,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cpu-stress-eval", "version": "0.1.0"},
        },
    })
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    read_responses(proc, mid)


def print_snapshot(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n{'=' * 60}")
    print(f"  {label}")
    print(f"{'=' * 60}")
    print(f"  CPU:       {d.get('cpu_usage_pct', 'N/A'):.1f}%")
    print(f"  RAM:       {d.get('ram_used_gb', 0):.2f} / {d.get('ram_total_gb', 0):.1f} GB")
    print(f"  RAM press: {d.get('ram_pressure', 'N/A')}")
    temp = d.get('die_temp_celsius')
    print(f"  Die temp:  {f'{temp:.1f}C' if temp else 'N/A'}")
    print(f"  Throttle:  {d.get('throttling', False)}")
    print(f"  Headroom:  {d.get('headroom', 'N/A')}")
    print(f"  Reason:    {d.get('headroom_reason', '')}")
    print(f"  Narrative: {data.get('narrative', '')}")


def print_blame(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n--- {label} ---")
    print(f"  Anomaly:   {d.get('anomaly_type', 'N/A')}")
    print(f"  Impact:    {d.get('impact_level', 'N/A')}")
    print(f"  Score:     {d.get('anomaly_score', 0):.3f}")
    c = d.get("culprit")
    if c:
        print(f"  Culprit:   {c.get('cmd', '?')} (PID {c.get('pid')}, CPU {c.get('cpu_pct', 0):.1f}%, RAM {c.get('ram_gb', 0):.2f}GB)")
    g = d.get("culprit_group")
    if g:
        print(f"  Group:     {g.get('name', '?')} ({g.get('process_count')} procs, CPU {g.get('total_cpu_pct', 0):.1f}%, RAM {g.get('total_ram_gb', 0):.2f}GB)")
    print(f"  Impact:    {d.get('impact', '')}")
    print(f"  Fix:       {d.get('fix', '')}")
    print(f"  Narrative: {data.get('narrative', '')}")


def print_session_health(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n{'=' * 60}")
    print(f"  {label}")
    print(f"{'=' * 60}")
    print(f"  Snapshots:     {d.get('snapshot_count', 0)}")
    print(f"  Alerts:        {d.get('alert_count', 0)}")
    print(f"  Worst Impact:  {d.get('worst_impact_level', 'N/A')}")
    print(f"  Worst Anomaly: {d.get('worst_anomaly_type', 'N/A')}")
    print(f"  Avg CPU:       {d.get('avg_cpu_pct', 0):.1f}%")
    print(f"  Peak CPU:      {d.get('peak_cpu_pct', 0):.1f}%")
    print(f"  Avg RAM:       {d.get('avg_ram_gb', 0):.2f} GB")
    print(f"  Peak RAM:      {d.get('peak_ram_gb', 0):.2f} GB")
    print(f"  Throttle evts: {d.get('throttle_event_count', 0)}")
    print(f"  Narrative:     {data.get('narrative', '')}")


def main() -> int:
    print("[info] CPU Stress Evaluation for Axon")
    print(f"[info] Binary: {AXON}")
    print(f"[info] Stress workers: {NUM_STRESS_WORKERS}")

    # Record session start for session_health query
    session_start = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())

    # Start axon serve
    proc = subprocess.Popen(
        [AXON, "serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    stress_procs: list[subprocess.Popen] = []

    try:
        initialize_mcp(proc)
        print("[ok] MCP initialized")

        # ── Verify all 7 MCP tools are registered ──
        tool_names = list_tools(proc)
        print(f"[info] MCP tools ({len(tool_names)}): {tool_names}")
        expected_tools = {"hw_snapshot", "process_blame", "battery_status",
                          "system_profile", "hardware_trend", "session_health",
                          "gpu_snapshot"}
        missing = expected_tools - set(tool_names)
        if missing:
            print(f"[FAIL] Missing MCP tools: {missing}")
            return 1
        print(f"[ok] All 7 MCP tools registered")

        # ── Phase 1: Baseline (let collector warm up 3+ ticks = 6s, wait 12s) ──
        print("\n[phase 1] Baseline -- waiting 12s for EWMA warmup...")
        time.sleep(12)

        baseline_hw = call_tool(proc, "hw_snapshot")
        baseline_blame = call_tool(proc, "process_blame")
        print_snapshot("BASELINE hw_snapshot", baseline_hw)
        print_blame("BASELINE process_blame", baseline_blame)

        # ── Phase 2: CPU Stress ──
        print(f"\n[phase 2] Starting {NUM_STRESS_WORKERS} CPU stress workers (yes > /dev/null)...")
        for i in range(NUM_STRESS_WORKERS):
            p = subprocess.Popen(
                ["yes"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            stress_procs.append(p)
            print(f"  [info] Worker {i+1} started (PID {p.pid})")

        # Let stress build up and axon detect it (need several ticks)
        print("[info] Waiting 16s for axon to detect CPU saturation...")
        time.sleep(16)

        # Take snapshots during stress
        stress_hw_1 = call_tool(proc, "hw_snapshot")
        stress_blame_1 = call_tool(proc, "process_blame")
        print_snapshot("STRESS hw_snapshot (t+16s)", stress_hw_1)
        print_blame("STRESS process_blame (t+16s)", stress_blame_1)

        # Wait more and take another reading
        print("[info] Waiting 14s more for sustained detection...")
        time.sleep(14)

        stress_hw_2 = call_tool(proc, "hw_snapshot")
        stress_blame_2 = call_tool(proc, "process_blame")
        print_snapshot("STRESS hw_snapshot (t+30s)", stress_hw_2)
        print_blame("STRESS process_blame (t+30s)", stress_blame_2)

        # ── Phase 3: Kill stress, observe recovery ──
        print(f"\n[phase 3] Killing stress workers...")
        for p in stress_procs:
            p.kill()
            p.wait()
        stress_procs.clear()
        print("[info] All stress workers killed. Waiting 12s for recovery...")
        time.sleep(12)

        recovery_hw = call_tool(proc, "hw_snapshot")
        recovery_blame = call_tool(proc, "process_blame")
        print_snapshot("RECOVERY hw_snapshot (12s post-stress)", recovery_hw)
        print_blame("RECOVERY process_blame (12s post-stress)", recovery_blame)

        # ── Phase 4: Session Health ──
        print(f"\n[phase 4] Querying session_health since {session_start}...")
        session = call_tool(proc, "session_health", {"since": session_start})
        print_session_health("SESSION HEALTH (entire evaluation)", session)

        # ── Phase 5: Hardware Trend ──
        print(f"\n[phase 5] Querying hardware_trend (last_1h, 1m buckets)...")
        trend = call_tool(proc, "hardware_trend", {"time_range": "last_1h", "interval": "1m"})
        td = trend.get("data", {})
        buckets = td.get("buckets", [])
        print(f"  Trend direction: {td.get('trend_direction', 'N/A')}")
        print(f"  Total snapshots: {td.get('total_snapshots', 0)}")
        print(f"  Buckets: {len(buckets)}")
        for b in buckets:
            ts = b.get("bucket_start", "?")[-8:]  # just time part
            print(f"    {ts}: CPU avg={b['cpu_avg']:.1f}% max={b['cpu_max']:.1f}% | RAM avg={b['ram_avg']:.2f}GB max={b['ram_max']:.2f}GB | anomalies={b['anomaly_count']} throttles={b['throttle_count']}")

        # ── Phase 6: GPU Snapshot ──
        print(f"\n[phase 6] Querying gpu_snapshot...")
        gpu = call_tool(proc, "gpu_snapshot")
        gpu_d = gpu.get("data", {})
        print(f"  OK:           {gpu.get('ok', False)}")
        print(f"  Detected:     {gpu_d.get('detected', False)}")
        print(f"  Model:        {gpu_d.get('model', 'N/A')}")
        print(f"  Utilization:  {gpu_d.get('utilization_pct', 'N/A')}")
        print(f"  Narrative:    {gpu.get('narrative', '')}")

        # ── Phase 7: Serve Lifecycle Check ──
        print(f"\n[phase 7] Checking serve process lifecycle...")
        serve_alive = proc.poll() is None
        print(f"  Serve alive:  {serve_alive} (pid={proc.pid})")

        # ── Summary Analysis ──
        print(f"\n{'=' * 60}")
        print("  EVALUATION SUMMARY")
        print(f"{'=' * 60}")

        b_cpu = baseline_hw.get("data", {}).get("cpu_usage_pct", 0)
        s_cpu = stress_hw_2.get("data", {}).get("cpu_usage_pct", 0)
        r_cpu = recovery_hw.get("data", {}).get("cpu_usage_pct", 0)
        print(f"  CPU baseline:  {b_cpu:.1f}%")
        print(f"  CPU peak:      {s_cpu:.1f}%")
        print(f"  CPU recovery:  {r_cpu:.1f}%")

        b_headroom = baseline_hw.get("data", {}).get("headroom", "?")
        s_headroom = stress_hw_2.get("data", {}).get("headroom", "?")
        r_headroom = recovery_hw.get("data", {}).get("headroom", "?")
        print(f"  Headroom:      {b_headroom} -> {s_headroom} -> {r_headroom}")

        b_anomaly = baseline_blame.get("data", {}).get("anomaly_type", "?")
        s_anomaly = stress_blame_2.get("data", {}).get("anomaly_type", "?")
        r_anomaly = recovery_blame.get("data", {}).get("anomaly_type", "?")
        print(f"  Anomaly:       {b_anomaly} -> {s_anomaly} -> {r_anomaly}")

        b_impact = baseline_blame.get("data", {}).get("impact_level", "?")
        s_impact = stress_blame_2.get("data", {}).get("impact_level", "?")
        r_impact = recovery_blame.get("data", {}).get("impact_level", "?")
        print(f"  Impact:        {b_impact} -> {s_impact} -> {r_impact}")

        s_alert_count = session.get("data", {}).get("alert_count", 0)
        print(f"  Alerts fired:  {s_alert_count}")

        # Validate expectations
        checks = []
        detected_stress = s_cpu > b_cpu + 20
        checks.append(("CPU spike detected", detected_stress, f"{b_cpu:.0f}% -> {s_cpu:.0f}%"))

        recovered = r_cpu < s_cpu - 10
        checks.append(("CPU recovered after kill", recovered, f"{s_cpu:.0f}% -> {r_cpu:.0f}%"))

        headroom_degraded = s_headroom == "insufficient"
        checks.append(("Headroom insufficient during stress", headroom_degraded, f"{s_headroom}"))

        # Headroom may stay "limited" after recovery if disk pressure persists — that's correct
        headroom_improved = r_headroom in ("adequate", "limited")
        checks.append(("Headroom improved after recovery", headroom_improved, f"{r_headroom}"))

        anomaly_detected = s_anomaly != "none"
        checks.append(("Anomaly detected during stress", anomaly_detected, f"{s_anomaly}"))

        # After recovery, anomaly may be "none" or "agent_accumulation" (if agents
        # are the top group at idle). Both are acceptable — the key is that
        # cpu_saturation is no longer reported.
        anomaly_cleared = r_anomaly != "cpu_saturation"
        checks.append(("CPU saturation cleared after recovery", anomaly_cleared, f"{r_anomaly}"))

        # Alert verification: alerts should fire during stress
        alerts_fired = s_alert_count > 0
        checks.append(("Alerts fired during stress", alerts_fired, f"alert_count={s_alert_count}"))

        # GPU snapshot returns valid JSON (ok=true even if no GPU detected)
        gpu_valid = gpu.get("ok") is True
        checks.append(("GPU snapshot returns valid JSON", gpu_valid, f"ok={gpu.get('ok')}"))

        # Serve process stayed alive throughout entire evaluation
        checks.append(("Serve process alive throughout eval", serve_alive, f"pid={proc.pid}"))

        print(f"\n  CHECKS:")
        all_pass = True
        for label, passed, detail in checks:
            status = "[pass]" if passed else "[FAIL]"
            if not passed:
                all_pass = False
            print(f"    {status} {label} ({detail})")

        print(f"\n  RESULT: {'ALL CHECKS PASSED' if all_pass else 'SOME CHECKS FAILED'}")
        return 0 if all_pass else 1

    finally:
        # Cleanup stress workers
        for p in stress_procs:
            try:
                p.kill()
                p.wait()
            except Exception:
                pass
        # Test clean exit: close stdin and wait for graceful shutdown
        if proc.stdin and not proc.stdin.closed:
            proc.stdin.close()
        try:
            proc.wait(timeout=5)
            print(f"[ok] Serve exited cleanly on stdin close (rc={proc.returncode})")
        except subprocess.TimeoutExpired:
            print("[warn] Serve did not exit on stdin close, terminating")
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()
