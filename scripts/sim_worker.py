#!/usr/bin/env python3
"""
Simulated agent worker.

Sets process name to "claude" via prctl so axon detects it as a Claude session,
then runs one of several failure profiles:
  alloc  -- steady memory growth (node-pty ArrayBuffer leak pattern)
  spin   -- CPU spin loop (V8 GC thrash / futex busy-wait)
  churn  -- rapid subprocess spawning (zombie storm, statusLine render bug)
  idle   -- flat memory, low CPU (stale session)

Usage:
  python3 sim_worker.py alloc 5.0   # 5 MB per 2-second tick
  python3 sim_worker.py spin        # max CPU
  python3 sim_worker.py churn       # spawn 30 children/sec
  python3 sim_worker.py idle        # do nothing, pretend to be a session

The worker exits after SIM_TIMEOUT_S seconds (default 90).
"""
import ctypes
import math
import os
import subprocess
import sys
import time

SIM_TIMEOUT_S = int(os.environ.get("SIM_TIMEOUT_S", "90"))

def set_proc_name(name: str):
    try:
        libc = ctypes.CDLL("libc.so.6")
        libc.prctl(15, name.encode(), 0, 0, 0)
    except Exception:
        pass

def profile_alloc(mb_per_tick: float):
    heap = []
    max_mb = 300  # safety cap
    total_mb = 0
    deadline = time.time() + SIM_TIMEOUT_S
    while time.time() < deadline:
        if total_mb < max_mb:
            chunk = bytearray(int(mb_per_tick * 1024 * 1024))
            # Touch every page so the OS actually maps it (RSS, not VSZ)
            for i in range(0, len(chunk), 4096):
                chunk[i] = 1
            heap.append(chunk)
            total_mb += mb_per_tick
        time.sleep(2.0)

def profile_spin():
    deadline = time.time() + SIM_TIMEOUT_S
    # Spin one core for SIM_TIMEOUT_S seconds
    while time.time() < deadline:
        _ = math.sqrt(3.14159265) ** 2  # cheap float work — full core usage

def profile_churn():
    deadline = time.time() + SIM_TIMEOUT_S
    while time.time() < deadline:
        # Spawn 30 very short-lived subprocesses in this 2-second window
        children = []
        for _ in range(30):
            p = subprocess.Popen(["true"], close_fds=True)
            children.append(p)
        for p in children:
            p.wait()
        time.sleep(2.0)

def profile_idle():
    time.sleep(SIM_TIMEOUT_S)

def main():
    mode = sys.argv[1] if len(sys.argv) > 1 else "idle"
    param = float(sys.argv[2]) if len(sys.argv) > 2 else 1.0

    # Rename process to "claude" so axon includes it in claude_agents
    set_proc_name("claude")

    if mode == "alloc":
        profile_alloc(param)
    elif mode == "spin":
        profile_spin()
    elif mode == "churn":
        profile_churn()
    else:
        profile_idle()

if __name__ == "__main__":
    main()
