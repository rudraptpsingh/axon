#!/usr/bin/env python3
"""
Axon Performance Showcase: Before/After Comparison

Demonstrates the value of Axon by running the same benchmark task under
stress with and without Axon, measuring detection time, resolution time,
and task completion impact.

Usage:
  python3 scripts/perf_test_scenario.py --axon-bin ./target/release/axon
  python3 scripts/perf_test_scenario.py --scenario cpu --axon-bin ./target/release/axon
  python3 scripts/perf_test_scenario.py --output results.json --axon-bin ./target/release/axon
"""
from __future__ import annotations

import argparse
import json
import os
import signal
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
STRESS_DIR = SCRIPT_DIR / "stress"
PROXY_TASK = SCRIPT_DIR / "perf_proxy_task.sh"
SCENARIOS = ["cpu", "memory", "combined", "disk"]


# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------

def run_proxy_task() -> float:
    """Run the proxy benchmark task, return wall-clock seconds."""
    r = subprocess.run(
        [str(PROXY_TASK)],
        capture_output=True, text=True, timeout=120,
    )
    if r.returncode != 0:
        print(f"[warn] proxy task failed: {r.stderr.strip()}", file=sys.stderr)
        return -1.0
    try:
        return float(r.stdout.strip())
    except ValueError:
        return -1.0


def avg_task_time(runs: int = 3) -> float:
    """Average proxy task time over N runs."""
    times = [run_proxy_task() for _ in range(runs)]
    valid = [t for t in times if t > 0]
    if not valid:
        return -1.0
    return sum(valid) / len(valid)


def start_cpu_stress() -> list[subprocess.Popen]:
    """Start CPU stress, return list of processes."""
    procs: list[subprocess.Popen] = []
    ncpu = os.cpu_count() or 4
    count = ncpu * 2
    for _ in range(count):
        p = subprocess.Popen(["yes"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        procs.append(p)
    for _ in range(ncpu):
        p = subprocess.Popen(
            ["dd", "if=/dev/urandom", "of=/dev/null", "bs=1M", "count=99999"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p)
    return procs


def start_memory_stress() -> subprocess.Popen:
    """Start memory stress, return process."""
    pid_file = "/tmp/axon_stress_mem.pids"
    p = subprocess.Popen(
        [sys.executable, str(STRESS_DIR / "mem_stress.py"), pid_file],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return p


def start_disk_stress() -> list[subprocess.Popen]:
    """Start disk stress, return list of processes."""
    procs: list[subprocess.Popen] = []
    target_dir = "/tmp/axon_disk_stress"
    os.makedirs(target_dir, exist_ok=True)
    for i in range(4):
        f = f"{target_dir}/stress_{i}.dat"
        p = subprocess.Popen(
            ["dd", "if=/dev/zero", f"of={f}", "bs=1M", "count=1024"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p)
    return procs


def kill_procs(procs: list[subprocess.Popen | None]) -> None:
    for p in procs:
        if p is None:
            continue
        try:
            p.terminate()
            p.wait(timeout=3)
        except (subprocess.TimeoutExpired, OSError):
            try:
                p.kill()
            except OSError:
                pass


def cleanup_disk_stress() -> None:
    import shutil
    shutil.rmtree("/tmp/axon_disk_stress", ignore_errors=True)


def start_stress(scenario: str) -> list[subprocess.Popen]:
    """Start stress for the given scenario. Returns all processes to kill later."""
    if scenario == "cpu":
        return start_cpu_stress()
    elif scenario == "memory":
        return [start_memory_stress()]
    elif scenario == "combined":
        cpu = start_cpu_stress()
        mem = start_memory_stress()
        return cpu + [mem]
    elif scenario == "disk":
        return start_disk_stress()
    else:
        raise ValueError(f"Unknown scenario: {scenario}")


def stop_stress(procs: list[subprocess.Popen], scenario: str) -> None:
    kill_procs(procs)
    if scenario in ("disk",):
        cleanup_disk_stress()


# ---------------------------------------------------------------------------
# Webhook receiver (for "with Axon" phase)
# ---------------------------------------------------------------------------

class WebhookCollector:
    def __init__(self) -> None:
        self.posts: list[dict[str, Any]] = []
        self.timestamps: list[float] = []
        self.lock = threading.Lock()
        self.server: HTTPServer | None = None
        self.port = 0
        self.url = ""

    def start(self) -> None:
        collector = self

        class H(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                ln = int(self.headers.get("Content-Length", 0))
                raw = self.rfile.read(ln)
                try:
                    obj = json.loads(raw)
                    with collector.lock:
                        collector.posts.append(obj)
                        collector.timestamps.append(time.time())
                except json.JSONDecodeError:
                    pass
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"ok")

            def log_message(self, *_: object) -> None:
                pass

        self.server = HTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}/alerts"
        t = threading.Thread(target=self.server.serve_forever, daemon=True)
        t.start()

    def stop(self) -> None:
        if self.server:
            self.server.shutdown()

    def wait_for_alert(self, timeout_s: float = 60.0) -> float | None:
        """Wait for first alert. Returns seconds since epoch of first alert, or None."""
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            with self.lock:
                if self.timestamps:
                    return self.timestamps[0]
            time.sleep(0.5)
        return None

    def get_alerts(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.posts)


# ---------------------------------------------------------------------------
# MCP client (for process_blame call)
# ---------------------------------------------------------------------------

def mcp_call_process_blame(axon_proc: subprocess.Popen) -> dict[str, Any] | None:
    """Send MCP initialize + process_blame, return parsed data or None."""
    if not axon_proc.stdin or not axon_proc.stdout:
        return None

    def send(obj: dict) -> None:
        assert axon_proc.stdin
        axon_proc.stdin.write(json.dumps(obj) + "\n")
        axon_proc.stdin.flush()

    def read_until_id(target_id: int, timeout_s: float = 30.0) -> dict | None:
        assert axon_proc.stdout
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            line = axon_proc.stdout.readline()
            if not line:
                break
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if msg.get("id") == target_id and "result" in msg:
                return msg
        return None

    # Initialize
    send({
        "jsonrpc": "2.0", "id": 0, "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "perf-test", "version": "0.1.0"},
        },
    })
    send({"jsonrpc": "2.0", "method": "notifications/initialized"})
    init_resp = read_until_id(0, timeout_s=15.0)
    if not init_resp:
        return None

    # Call process_blame
    send({
        "jsonrpc": "2.0", "id": 1, "method": "tools/call",
        "params": {"name": "process_blame", "arguments": {}},
    })
    blame_resp = read_until_id(1, timeout_s=30.0)
    if not blame_resp:
        return None

    try:
        text = blame_resp["result"]["content"][0]["text"]
        return json.loads(text)
    except (KeyError, IndexError, json.JSONDecodeError):
        return None


# ---------------------------------------------------------------------------
# Scenario runner
# ---------------------------------------------------------------------------

def run_scenario(scenario: str, axon_bin: str) -> dict[str, Any]:
    """Run a single before/after scenario. Returns metrics dict."""
    print(f"\n{'=' * 60}")
    print(f"  SCENARIO: {scenario.upper()}")
    print(f"{'=' * 60}")

    result: dict[str, Any] = {"scenario": scenario, "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S")}

    # --- Act 1: Baseline ---
    print("\n[phase 1/3] BASELINE -- measuring idle task performance...")
    baseline_time = avg_task_time(3)
    result["baseline_task_time"] = round(baseline_time, 2)
    print(f"  Baseline task time: {baseline_time:.2f}s (avg of 3 runs)")

    # --- Act 2: Without Axon ---
    print("\n[phase 2/3] WITHOUT AXON -- blind execution under stress...")
    stress_procs = start_stress(scenario)
    time.sleep(8)  # let stress ramp up

    stress_start = time.time()
    blind_task_time = run_proxy_task()
    blind_task_end = time.time()

    result["blind_task_time"] = round(blind_task_time, 2)
    result["blind_slowdown_factor"] = round(blind_task_time / baseline_time, 1) if baseline_time > 0 else -1
    result["blind_alerts"] = 0
    result["blind_diagnosis"] = "none"
    result["blind_fix_applied"] = False

    print(f"  Task time under stress: {blind_task_time:.2f}s ({result['blind_slowdown_factor']}x slower)")
    print(f"  Alerts: 0 (no hardware awareness)")
    print(f"  Diagnosis: none (agent unaware of root cause)")

    stop_stress(stress_procs, scenario)
    print("  [cooldown] waiting 10s for system recovery...")
    time.sleep(10)

    # --- Act 3: With Axon ---
    print("\n[phase 3/3] WITH AXON -- informed execution under stress...")

    webhook = WebhookCollector()
    webhook.start()

    # Set up alert config
    cfg_dir = tempfile.mkdtemp(prefix="axon_perf_test_")
    cfg = {
        "channels": [{
            "type": "webhook",
            "id": "perf_test",
            "url": webhook.url,
            "filters": {"severity": [], "alert_types": ["*"]},
        }]
    }
    cfg_path = Path(cfg_dir) / "alert-dispatch.json"
    cfg_path.write_text(json.dumps(cfg), encoding="utf-8")

    data_dir = tempfile.mkdtemp(prefix="axon_perf_data_")
    env = os.environ.copy()
    env["AXON_CONFIG_DIR"] = cfg_dir
    env["AXON_DATA_DIR"] = data_dir
    # Inject test state for deterministic edge-triggered alerts
    env["AXON_TEST_PREV_RAM_PRESSURE"] = "normal"
    env["AXON_TEST_PREV_IMPACT_LEVEL"] = "healthy"
    env["AXON_TEST_PREV_THROTTLING"] = "0"
    env["AXON_TEST_PRESERVE_PREV_DURING_WARMUP"] = "1"

    # Start axon serve with MCP over stdio
    axon_proc = subprocess.Popen(
        [axon_bin, "serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
        env=env,
    )

    # Wait for collector warm-up (3 ticks = ~6s)
    time.sleep(8)

    # Start stress
    stress_procs = start_stress(scenario)
    stress_start_axon = time.time()
    print(f"  Stress started, waiting for Axon detection...")

    # Wait for first alert
    first_alert_time = webhook.wait_for_alert(timeout_s=60.0)
    mttd = round(first_alert_time - stress_start_axon, 1) if first_alert_time else -1
    result["mttd_seconds"] = mttd

    if mttd > 0:
        print(f"  [alert] First alert at +{mttd}s (MTTD)")
    else:
        print(f"  [warn] No alert received in 60s")

    # Call process_blame via MCP
    blame_data = mcp_call_process_blame(axon_proc)
    blame_time = time.time()
    result["blame_data"] = blame_data

    blame_correct = False
    fix_suggestion = ""
    if blame_data and blame_data.get("ok"):
        data = blame_data.get("data", {})
        culprit = data.get("culprit_group", {}) or data.get("culprit", {})
        culprit_name = culprit.get("name", "unknown")
        fix_suggestion = data.get("fix", "")
        impact_level = data.get("impact_level", "unknown")
        anomaly_type = data.get("anomaly_type", "unknown")

        # Check if blame is correct (stress process identified)
        stress_names = {
            "cpu": ["yes", "dd"],
            "memory": ["mem_stress", "python", "python3"],
            "combined": ["yes", "dd", "mem_stress", "python", "python3"],
            "disk": ["dd"],
        }
        expected = stress_names.get(scenario, [])
        blame_correct = any(e in culprit_name.lower() for e in expected)

        print(f"  [blame] Culprit: {culprit_name} | Impact: {impact_level} | Anomaly: {anomaly_type}")
        print(f"  [blame] Correct: {blame_correct}")
        print(f"  [fix]   {fix_suggestion}")
    else:
        print(f"  [warn] process_blame returned no data")

    result["blame_correct"] = blame_correct
    result["fix_suggestion"] = fix_suggestion

    # Apply fix: kill stress (simulates agent following Axon's advice)
    fix_start = time.time()
    stop_stress(stress_procs, scenario)
    fix_time = time.time()

    mttr = round(fix_time - (first_alert_time or stress_start_axon), 1) if first_alert_time else -1
    result["mttr_seconds"] = mttr
    print(f"  [fix] Stress killed (MTTR: {mttr}s from first alert)")

    # Wait briefly for recovery, then measure task time
    time.sleep(5)
    recovered_task_time = run_proxy_task()
    result["recovered_task_time"] = round(recovered_task_time, 2)
    result["recovered_factor"] = round(recovered_task_time / baseline_time, 1) if baseline_time > 0 else -1

    print(f"  [recovery] Task time after fix: {recovered_task_time:.2f}s ({result['recovered_factor']}x of baseline)")

    # Collect all alerts
    alerts = webhook.get_alerts()
    result["alert_count"] = len(alerts)
    result["alert_types"] = list({a.get("alert_type", "unknown") for a in alerts})
    result["alert_severities"] = list({a.get("severity", "unknown") for a in alerts})
    result["webhook_payloads"] = alerts[:5]  # keep first 5

    print(f"  [alerts] Total: {len(alerts)}, Types: {result['alert_types']}")

    # Cleanup
    axon_proc.terminate()
    try:
        axon_proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        axon_proc.kill()
    webhook.stop()

    import shutil
    shutil.rmtree(cfg_dir, ignore_errors=True)
    shutil.rmtree(data_dir, ignore_errors=True)

    # --- Summary ---
    result["passed"] = all([
        mttd > 0 and mttd < 30,
        result.get("recovered_factor", 99) < 2.0,
        len(alerts) > 0,
    ])

    return result


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def get_system_info() -> dict[str, Any]:
    """Gather basic machine info for the report header."""
    import platform
    info: dict[str, Any] = {
        "platform": platform.system(),
        "machine": platform.machine(),
        "cpu_count": os.cpu_count(),
    }
    try:
        with open("/proc/meminfo") as f:
            for line in f:
                if line.startswith("MemTotal:"):
                    kb = int(line.split()[1])
                    info["ram_total_gb"] = round(kb / 1024 / 1024, 1)
                    break
    except FileNotFoundError:
        # macOS
        try:
            r = subprocess.run(["sysctl", "-n", "hw.memsize"], capture_output=True, text=True)
            if r.returncode == 0:
                info["ram_total_gb"] = round(int(r.stdout.strip()) / 1024 / 1024 / 1024, 1)
        except FileNotFoundError:
            pass
    return info


def main() -> int:
    ap = argparse.ArgumentParser(description="Axon Performance Showcase")
    ap.add_argument("--axon-bin", required=True, type=str, help="Path to axon binary")
    ap.add_argument("--scenario", choices=SCENARIOS + ["all"], default="all",
                    help="Which stress scenario to run (default: all)")
    ap.add_argument("--output", metavar="PATH", help="Write results JSON to file")
    args = ap.parse_args()

    axon_bin = args.axon_bin
    if not Path(axon_bin).is_file():
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        print("  Run: cargo build --release -p axon", file=sys.stderr)
        return 2

    scenarios = SCENARIOS if args.scenario == "all" else [args.scenario]

    print("=" * 60)
    print("  AXON PERFORMANCE SHOWCASE")
    print("=" * 60)

    system_info = get_system_info()
    print(f"Machine: {system_info.get('platform')} {system_info.get('machine')}, "
          f"{system_info.get('cpu_count')} cores, "
          f"{system_info.get('ram_total_gb', '?')}GB RAM")
    print(f"Date: {time.strftime('%Y-%m-%d %H:%M')}")
    print(f"Scenarios: {', '.join(scenarios)}")

    results: list[dict[str, Any]] = []
    for scenario in scenarios:
        try:
            r = run_scenario(scenario, axon_bin)
            results.append(r)
        except Exception as e:
            print(f"\n[err] Scenario {scenario} failed: {e}", file=sys.stderr)
            results.append({"scenario": scenario, "error": str(e), "passed": False})
        finally:
            # Always cleanup stress
            subprocess.run([str(STRESS_DIR / "cleanup.sh")], capture_output=True)

    # Print summary
    print(f"\n{'=' * 60}")
    print(f"  SUMMARY")
    print(f"{'=' * 60}")
    print(f"{'Scenario':<12} {'MTTD':>6} {'MTTR':>6} {'Blind':>8} {'Fixed':>8} {'Alerts':>7} {'Blame':>6} {'Pass':>5}")
    print("-" * 60)
    for r in results:
        if "error" in r:
            print(f"{r['scenario']:<12} {'ERROR':>6}")
            continue
        mttd = f"{r.get('mttd_seconds', -1)}s"
        mttr = f"{r.get('mttr_seconds', -1)}s"
        blind = f"{r.get('blind_slowdown_factor', '?')}x"
        fixed = f"{r.get('recovered_factor', '?')}x"
        alerts = str(r.get("alert_count", 0))
        blame = "yes" if r.get("blame_correct") else "no"
        passed = "yes" if r.get("passed") else "NO"
        print(f"{r['scenario']:<12} {mttd:>6} {mttr:>6} {blind:>8} {fixed:>8} {alerts:>7} {blame:>6} {passed:>5}")

    all_passed = all(r.get("passed", False) for r in results)
    print(f"\nOverall: {'ALL PASSED' if all_passed else 'SOME FAILED'}")

    # Write output
    output_data = {
        "system_info": system_info,
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "scenarios": results,
        "all_passed": all_passed,
    }

    if args.output:
        Path(args.output).write_text(json.dumps(output_data, indent=2, default=str))
        print(f"\nResults written to {args.output}")

    # Always write to stdout-friendly JSON on a separate line for piping
    print(f"\n--- JSON ---")
    print(json.dumps(output_data, indent=2, default=str))

    return 0 if all_passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
