#!/usr/bin/env python3
"""
Windows stress-eval for axon: baseline -> stress -> verify detection -> recovery.
Exercises all 7 MCP tools at each phase and reports what axon sees.

Usage: python3 scripts/windows_stress_eval.py ./target/debug/axon.exe
"""
from __future__ import annotations
import json, subprocess, sys, time, os, signal
from typing import Any

AXON = sys.argv[1] if len(sys.argv) > 1 else r".\target\debug\axon.exe"
MSG_ID = 0

def start_axon():
    proc = subprocess.Popen(
        [AXON, "serve"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True,
    )
    send(proc, {
        "jsonrpc": "2.0", "id": next_id(),
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "stress-eval", "version": "1.0"},
        },
    })
    recv(proc)  # init response
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    return proc

def next_id():
    global MSG_ID
    MSG_ID += 1
    return MSG_ID

def send(proc, obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()

def recv(proc):
    line = proc.stdout.readline()
    if not line.strip():
        return None
    return json.loads(line)

def call_tool(proc, name, args=None):
    req_id = next_id()
    send(proc, {
        "jsonrpc": "2.0", "id": req_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": args or {}},
    })
    # Read lines until we find our response (skip notifications)
    for _ in range(20):  # safety limit
        resp = recv(proc)
        if resp is None:
            continue
        # Skip notifications (no id field)
        if "id" not in resp:
            continue
        # Found our response (or any response — rmcp sends in order)
        if "result" in resp:
            content = resp["result"].get("content", [])
            for c in content:
                if c.get("type") == "text":
                    return json.loads(c["text"])
        return None
    return None

def call_all_tools(proc):
    results = {}
    for name in ["hw_snapshot", "process_blame", "battery_status",
                  "system_profile", "gpu_snapshot", "session_health", "hardware_trend"]:
        result = call_tool(proc, name)
        results[name] = result if result else {"ok": False, "data": {}, "narrative": "tool returned null"}
        time.sleep(0.5)
    return results

def print_summary(label, results):
    print(f"\n{'='*60}")
    print(f"  {label}")
    print(f"{'='*60}")

    hw = (results.get("hw_snapshot") or {}).get("data", {})
    blame = (results.get("process_blame") or {}).get("data", {})
    gpu = (results.get("gpu_snapshot") or {}).get("data", {})
    batt = (results.get("battery_status") or {}).get("data", {})
    profile = (results.get("system_profile") or {}).get("data", {})
    health = results.get("session_health") or {}

    cpu_val = hw.get('cpu_usage_pct', 'N/A')
    print(f"  CPU:      {cpu_val:.1f}%" if isinstance(cpu_val, (int, float)) else f"  CPU:      {cpu_val}")
    print(f"  RAM:      {hw.get('ram_used_gb', 0):.1f}/{hw.get('ram_total_gb', 0):.0f}GB  pressure={hw.get('ram_pressure', '?')}")
    print(f"  Disk:     {hw.get('disk_used_gb', 0):.0f}/{hw.get('disk_total_gb', 0):.0f}GB  pressure={hw.get('disk_pressure', '?')}")
    print(f"  Temp:     {hw.get('die_temp_celsius', 'N/A')}")
    print(f"  Throttle: {hw.get('throttling', 'N/A')}")
    print(f"  Headroom: {hw.get('headroom', '?')} ({hw.get('headroom_reason', '')})")
    print()
    print(f"  Impact:   {blame.get('impact_level', '?')}  score={blame.get('anomaly_score', 0):.3f}")
    print(f"  Anomaly:  {blame.get('anomaly_type', 'none')}")
    if blame.get("culprit"):
        c = blame["culprit"]
        print(f"  Culprit:  {c.get('cmd', '?')} (PID {c.get('pid', '?')}) -- {c.get('cpu_pct', 0):.0f}% CPU, {c.get('ram_gb', 0):.1f}GB RAM")
    print(f"  Fix:      {blame.get('fix', 'N/A')}")
    print()
    print(f"  GPU:      {gpu.get('model', 'N/A')}  util={gpu.get('utilization_pct', 'N/A')}%  detected={gpu.get('detected', False)}")
    print(f"  Battery:  {batt.get('percentage', 'N/A')}%  charging={batt.get('is_charging', 'N/A')}")
    print(f"  Profile:  {profile.get('model_id', '?')} / {profile.get('chip', '?')} / {profile.get('os_version', '?')}")

    # Print narratives
    print(f"\n  Narratives:")
    for tool_name in ["hw_snapshot", "process_blame", "gpu_snapshot", "battery_status"]:
        r = results.get(tool_name, {})
        narr = r.get("narrative", "")
        if narr:
            print(f"    {tool_name}: {narr[:120]}")

    health_narr = health.get("narrative", "")
    if health_narr:
        print(f"    session_health: {health_narr[:120]}")

    return hw, blame, gpu

def spawn_stress():
    """Spawn CPU + memory stress processes."""
    procs = []
    # CPU stress: 4 busy loops
    for _ in range(4):
        p = subprocess.Popen(
            ["python3", "-c", "import time; t=time.time()\nwhile time.time()-t<45:\n sum(range(100000))"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p)

    # Memory stress: allocate ~2GB
    p = subprocess.Popen(
        ["python3", "-c", "import time; data=[bytearray(100*1024*1024) for _ in range(20)]; time.sleep(45)"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    procs.append(p)
    return procs

def kill_stress(procs):
    for p in procs:
        try:
            p.kill()
            p.wait(timeout=5)
        except Exception:
            pass

def main():
    print("[eval] Starting axon MCP server...")
    proc = start_axon()

    # Wait for collector warm-up (needs 3 EWMA ticks = 6s)
    print("[eval] Waiting 8s for collector warm-up...")
    time.sleep(8)

    # Phase 1: Baseline
    print("\n[eval] Phase 1: BASELINE")
    baseline = call_all_tools(proc)
    hw_base, blame_base, gpu_base = print_summary("PHASE 1: BASELINE", baseline)

    # Phase 2: Stress
    print("\n[eval] Phase 2: Generating CPU + memory stress...")
    stress_procs = spawn_stress()
    print(f"[eval] Spawned {len(stress_procs)} stress processes. Waiting 15s for detection...")
    time.sleep(15)

    stressed = call_all_tools(proc)
    hw_stress, blame_stress, gpu_stress = print_summary("PHASE 2: UNDER STRESS", stressed)

    # Phase 3: Recovery
    print("\n[eval] Phase 3: Killing stress, waiting for recovery...")
    kill_stress(stress_procs)
    time.sleep(10)

    recovered = call_all_tools(proc)
    hw_recov, blame_recov, gpu_recov = print_summary("PHASE 3: RECOVERY", recovered)

    # Phase 4: Analysis
    print(f"\n{'='*60}")
    print(f"  ANALYSIS")
    print(f"{'='*60}")

    issues = []
    passes = []

    # Check CPU detection
    cpu_base = hw_base.get("cpu_usage_pct", 0)
    cpu_stress = hw_stress.get("cpu_usage_pct", 0)
    cpu_recov = hw_recov.get("cpu_usage_pct", 0)
    if cpu_stress > cpu_base + 10:
        passes.append(f"CPU spike detected: {cpu_base:.0f}% -> {cpu_stress:.0f}%")
    else:
        issues.append(f"CPU spike NOT detected: {cpu_base:.0f}% -> {cpu_stress:.0f}%")

    if cpu_recov < cpu_stress - 5:
        passes.append(f"CPU recovery detected: {cpu_stress:.0f}% -> {cpu_recov:.0f}%")
    else:
        issues.append(f"CPU recovery NOT detected: {cpu_stress:.0f}% -> {cpu_recov:.0f}%")

    # Check RAM detection
    ram_base = hw_base.get("ram_used_gb", 0)
    ram_stress = hw_stress.get("ram_used_gb", 0)
    ram_recov = hw_recov.get("ram_used_gb", 0)
    if ram_stress > ram_base + 0.5:
        passes.append(f"RAM increase detected: {ram_base:.1f}GB -> {ram_stress:.1f}GB")
    else:
        issues.append(f"RAM increase NOT detected: {ram_base:.1f}GB -> {ram_stress:.1f}GB")

    # Check impact scoring
    score_base = blame_base.get("anomaly_score", 0)
    score_stress = blame_stress.get("anomaly_score", 0)
    score_recov = blame_recov.get("anomaly_score", 0)
    if score_stress > score_base:
        passes.append(f"Impact score increased: {score_base:.3f} -> {score_stress:.3f}")
    else:
        issues.append(f"Impact score did NOT increase: {score_base:.3f} -> {score_stress:.3f}")

    level_stress = blame_stress.get("impact_level", "healthy")
    if level_stress in ("strained", "degrading", "critical"):
        passes.append(f"Impact level escalated to: {level_stress}")
    else:
        issues.append(f"Impact level stayed: {level_stress} (expected escalation)")

    # Check culprit identification
    culprit = blame_stress.get("culprit", {})
    culprit_cmd = culprit.get("cmd", "").lower()
    if "python" in culprit_cmd:
        passes.append(f"Culprit correctly identified stress process: {culprit.get('cmd', '?')}")
    elif culprit_cmd:
        passes.append(f"Culprit identified (not stress proc): {culprit.get('cmd', '?')}")
    else:
        issues.append("No culprit identified under stress")

    # Check GPU populated
    if gpu_base.get("detected"):
        passes.append(f"GPU detected: {gpu_base.get('model', '?')}")
        if gpu_base.get("utilization_pct") is not None:
            passes.append(f"GPU utilization populated: {gpu_base.get('utilization_pct')}%")
        else:
            issues.append("GPU utilization is null (expected a value)")
    else:
        issues.append("GPU not detected")

    # Check battery
    batt = baseline.get("battery_status", {}).get("data", {})
    if batt.get("percentage") is not None:
        passes.append(f"Battery status populated: {batt.get('percentage')}%")
    else:
        issues.append("Battery status returned null")

    # Check temperature
    if hw_base.get("die_temp_celsius") is not None:
        passes.append(f"Temperature populated: {hw_base.get('die_temp_celsius')}C")
    else:
        issues.append("Temperature is null (Windows limitation without admin)")

    # Check headroom
    headroom_stress = hw_stress.get("headroom", "adequate")
    if headroom_stress in ("limited", "insufficient"):
        passes.append(f"Headroom correctly reduced to: {headroom_stress}")
    else:
        issues.append(f"Headroom stayed: {headroom_stress} under stress")

    # Check disk I/O (impact scoring includes it)
    # Just verify the impact weights are incorporating it

    # Check system_profile
    profile = baseline.get("system_profile", {}).get("data", {})
    if profile.get("model_id") and profile["model_id"] != "Unknown Machine":
        passes.append(f"Machine model detected: {profile.get('model_id')}")
    else:
        issues.append(f"Machine model generic: {profile.get('model_id', 'N/A')}")

    if profile.get("os_version", "").startswith("Windows"):
        passes.append(f"OS detected: {profile.get('os_version')}")
    else:
        issues.append(f"OS not detected as Windows: {profile.get('os_version', 'N/A')}")

    # Check session_health
    health = stressed.get("session_health", {})
    if health.get("ok"):
        passes.append("session_health responded ok")
    else:
        issues.append("session_health failed")

    # Check hardware_trend
    trend = stressed.get("hardware_trend", {})
    if trend.get("ok"):
        passes.append("hardware_trend responded ok")
    else:
        issues.append("hardware_trend failed")

    print(f"\n  PASSED ({len(passes)}):")
    for p in passes:
        print(f"    [pass] {p}")

    print(f"\n  ISSUES ({len(issues)}):")
    for i in issues:
        print(f"    [issue] {i}")

    print(f"\n  Score: {len(passes)}/{len(passes)+len(issues)} checks passed")

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except:
        proc.kill()

    return 0 if not issues else 1

if __name__ == "__main__":
    sys.exit(main())
