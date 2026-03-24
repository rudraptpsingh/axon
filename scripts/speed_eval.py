#!/usr/bin/env python3
"""
Speed evaluation for axon MCP tools.
Measures response latency for each tool over multiple iterations.
Compares ring-buffer-backed tools vs DB-backed tools.

Usage: python3 scripts/speed_eval.py ./target/debug/axon.exe
"""
from __future__ import annotations
import json, subprocess, sys, time, statistics
from typing import Any

AXON = sys.argv[1] if len(sys.argv) > 1 else r".\target\debug\axon.exe"
MSG_ID = 0

def next_id():
    global MSG_ID
    MSG_ID += 1
    return MSG_ID

def send(proc, obj):
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()

def recv(proc):
    for _ in range(20):
        line = proc.stdout.readline()
        if not line.strip():
            continue
        resp = json.loads(line)
        if "id" not in resp:
            continue  # skip notifications
        return resp
    return None

def call_tool(proc, name, args=None):
    req_id = next_id()
    send(proc, {
        "jsonrpc": "2.0", "id": req_id,
        "method": "tools/call",
        "params": {"name": name, "arguments": args or {}},
    })
    t0 = time.perf_counter()
    resp = recv(proc)
    t1 = time.perf_counter()
    latency_ms = (t1 - t0) * 1000
    ok = False
    if resp and "result" in resp:
        content = resp["result"].get("content", [])
        for c in content:
            if c.get("type") == "text":
                data = json.loads(c["text"])
                ok = data.get("ok", False)
    return latency_ms, ok

def measure_tool(proc, name, args=None, iterations=10):
    latencies = []
    for _ in range(iterations):
        ms, ok = call_tool(proc, name, args)
        if ok:
            latencies.append(ms)
        time.sleep(0.1)  # small gap between calls
    return latencies

def print_stats(name, latencies, data_source):
    if not latencies:
        print(f"  {name:25s}  FAILED (0 successful calls)")
        return
    avg = statistics.mean(latencies)
    med = statistics.median(latencies)
    p95 = sorted(latencies)[int(len(latencies) * 0.95)]
    mn = min(latencies)
    mx = max(latencies)
    print(f"  {name:25s}  avg={avg:6.1f}ms  p50={med:6.1f}ms  p95={p95:6.1f}ms  min={mn:5.1f}ms  max={mx:5.1f}ms  [{data_source}]")

def main():
    print("[speed-eval] Starting axon MCP server...")
    proc = subprocess.Popen(
        [AXON, "serve"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        text=True,
    )

    # Initialize
    send(proc, {
        "jsonrpc": "2.0", "id": next_id(),
        "method": "initialize",
        "params": {
            "protocolVersion": "2024-11-05",
            "capabilities": {},
            "clientInfo": {"name": "speed-eval", "version": "1.0"},
        },
    })
    recv(proc)
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})

    # Wait for collector to warm up (need data in ring buffer)
    print("[speed-eval] Warming up collector (10s)...")
    time.sleep(10)

    ITERATIONS = 20
    print(f"\n[speed-eval] Measuring {ITERATIONS} iterations per tool...\n")
    print(f"{'='*90}")
    print(f"  {'Tool':25s}  {'avg':>8s}  {'p50':>8s}  {'p95':>8s}  {'min':>7s}  {'max':>7s}  Source")
    print(f"{'='*90}")

    # Point-in-time tools (AppState mutex — should be <1ms)
    print("\n  --- Point-in-time tools (AppState mutex) ---")
    for name in ["hw_snapshot", "process_blame", "battery_status", "system_profile", "gpu_snapshot"]:
        lats = measure_tool(proc, name, iterations=ITERATIONS)
        print_stats(name, lats, "AppState")

    # session_health — ring buffer fast path (default 1h window)
    print("\n  --- session_health (ring buffer fast path) ---")
    lats = measure_tool(proc, "session_health", iterations=ITERATIONS)
    print_stats("session_health (default)", lats, "Ring+DB COUNT")

    # session_health with explicit short window
    since_30m = (time.time() - 1800)
    from datetime import datetime, timezone
    since_str = datetime.fromtimestamp(since_30m, tz=timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
    lats = measure_tool(proc, "session_health", args={"since": since_str}, iterations=ITERATIONS)
    print_stats("session_health (30m)", lats, "Ring+DB COUNT")

    # hardware_trend — ring fast path (last_1h)
    print("\n  --- hardware_trend (ring vs DB) ---")
    lats = measure_tool(proc, "hardware_trend", args={"time_range": "last_1h", "interval": "1m"}, iterations=ITERATIONS)
    print_stats("trend last_1h/1m", lats, "Ring")

    lats = measure_tool(proc, "hardware_trend", args={"time_range": "last_1h", "interval": "5m"}, iterations=ITERATIONS)
    print_stats("trend last_1h/5m", lats, "Ring")

    # hardware_trend — DB path (last_24h, last_7d)
    lats = measure_tool(proc, "hardware_trend", args={"time_range": "last_24h", "interval": "15m"}, iterations=ITERATIONS)
    print_stats("trend last_24h/15m", lats, "DB")

    lats = measure_tool(proc, "hardware_trend", args={"time_range": "last_7d", "interval": "1h"}, iterations=ITERATIONS)
    print_stats("trend last_7d/1h", lats, "DB")

    print(f"\n{'='*90}")
    print(f"\n[speed-eval] Done.")

    proc.terminate()
    try:
        proc.wait(timeout=5)
    except:
        proc.kill()

if __name__ == "__main__":
    main()
