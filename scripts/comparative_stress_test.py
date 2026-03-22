#!/usr/bin/env python3
"""
Claude + Axon: Parallel Agent Intelligence Demo

Demonstrates the value of Axon by running 4 simulated Claude agents under
two scenarios:

  Scenario A — "Blind Agents": All 4 launch simultaneously, no hardware awareness.
    Reproduces real issues: #15487 (I/O storm), #17563 (thermal throttle),
    #11122 (process accumulation), #21403 (OOM with parallel sub-agents).

  Scenario B — "Axon-Informed Agents": Each agent queries Axon MCP tools before
    and during work, deferring when the system is stressed.

Usage:
  python3 scripts/comparative_stress_test.py --axon-bin ./target/release/axon
  python3 scripts/comparative_stress_test.py --axon-bin ./target/release/axon --duration 30
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path
from typing import Any

SCRIPT_DIR = Path(__file__).resolve().parent
STRESS_DIR = SCRIPT_DIR / "stress"

# Import our supporting modules (same directory)
sys.path.insert(0, str(SCRIPT_DIR))
from axon_aware_workload_runner import AgentWorkloadRunner, AxonMCPClient  # noqa: E402
from metrics_collector import MetricsCollector  # noqa: E402
from stress_report_generator import generate_report  # noqa: E402
from webhook_receiver import WebhookCollector  # noqa: E402


# ---------------------------------------------------------------------------
# Agent task definitions
# ---------------------------------------------------------------------------

def get_agent_tasks(duration: float) -> list[dict[str, Any]]:
    """Define the 4 agent tasks.  Each has a name, description, and command."""
    ncpu = os.cpu_count() or 4
    return [
        {
            "name": "Build Agent",
            "task": "Compile project (CPU-heavy)",
            "type": "cpu",
            "start_fn": lambda: _start_cpu_stress(ncpu),
        },
        {
            "name": "Test Agent",
            "task": "Run test suite (CPU + RAM)",
            "type": "cpu_light",
            "start_fn": lambda: _start_cpu_light_stress(ncpu),
        },
        {
            "name": "Analysis Agent",
            "task": "Codebase analysis (I/O + CPU)",
            "type": "disk",
            "start_fn": _start_disk_stress,
        },
        {
            "name": "Data Agent",
            "task": "Process large dataset (RAM-heavy)",
            "type": "memory",
            "start_fn": _start_memory_stress,
        },
    ]


# ---------------------------------------------------------------------------
# Stress helpers (follow perf_test_scenario.py patterns exactly)
# ---------------------------------------------------------------------------

def _start_cpu_stress(ncpu: int) -> list[subprocess.Popen]:
    """Spawn yes + dd processes to saturate CPU."""
    procs: list[subprocess.Popen] = []
    for _ in range(ncpu * 2):
        p = subprocess.Popen(["yes"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        procs.append(p)
    for _ in range(ncpu):
        p = subprocess.Popen(
            ["dd", "if=/dev/urandom", "of=/dev/null", "bs=1M", "count=99999"],
            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
        )
        procs.append(p)
    return procs


def _start_cpu_light_stress(ncpu: int) -> list[subprocess.Popen]:
    """Lighter CPU stress (simulates test suite — fewer processes)."""
    procs: list[subprocess.Popen] = []
    for _ in range(ncpu):
        p = subprocess.Popen(["yes"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        procs.append(p)
    return procs


def _start_memory_stress() -> list[subprocess.Popen]:
    """Start memory stress via existing script."""
    pid_file = "/tmp/axon_stress_mem.pids"
    p = subprocess.Popen(
        [sys.executable, str(STRESS_DIR / "mem_stress.py"), pid_file],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return [p]


def _start_disk_stress() -> list[subprocess.Popen]:
    """Start disk stress via dd writes."""
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


def kill_procs(procs: list[subprocess.Popen]) -> None:
    """Gracefully terminate processes (3s timeout, fallback kill)."""
    for p in procs:
        try:
            p.terminate()
            p.wait(timeout=3)
        except (subprocess.TimeoutExpired, OSError):
            try:
                p.kill()
            except OSError:
                pass


def cleanup_all() -> None:
    """Run cleanup script + remove disk stress files."""
    cleanup_sh = STRESS_DIR / "cleanup.sh"
    if cleanup_sh.exists():
        subprocess.run(["bash", str(cleanup_sh)], capture_output=True)
    shutil.rmtree("/tmp/axon_disk_stress", ignore_errors=True)


# ---------------------------------------------------------------------------
# Background collector management
# ---------------------------------------------------------------------------

def start_bg_collectors(output_dir: Path, duration: float) -> tuple[subprocess.Popen, subprocess.Popen]:
    """Start metrics_collector and responsiveness_tester as background processes."""
    metrics_proc = subprocess.Popen(
        [sys.executable, str(SCRIPT_DIR / "metrics_collector.py"),
         str(output_dir / "metrics.json"),
         "--interval", "2", "--duration", str(duration + 20)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    resp_proc = subprocess.Popen(
        [sys.executable, str(SCRIPT_DIR / "responsiveness_tester.py"),
         str(output_dir / "responsiveness.json"),
         "--interval", "5", "--duration", str(duration + 20)],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL,
    )
    return metrics_proc, resp_proc


def stop_bg_collectors(metrics_proc: subprocess.Popen, resp_proc: subprocess.Popen) -> None:
    """Stop background collector processes."""
    for p in (metrics_proc, resp_proc):
        try:
            p.terminate()
            p.wait(timeout=5)
        except (subprocess.TimeoutExpired, OSError):
            try:
                p.kill()
            except OSError:
                pass


# ---------------------------------------------------------------------------
# Scenario A: Blind Agents
# ---------------------------------------------------------------------------

def run_scenario_a(agents: list[dict], duration: float, output_dir: Path) -> list[dict[str, Any]]:
    """Run all agents simultaneously with NO Axon awareness."""
    output_dir.mkdir(parents=True, exist_ok=True)

    print("\n" + "=" * 60)
    print("  SCENARIO A: BLIND AGENTS (no hardware awareness)")
    print("=" * 60)
    print(f"  Launching {len(agents)} agents simultaneously...")
    print(f"  Duration: {duration}s")

    # Start background collectors
    metrics_proc, resp_proc = start_bg_collectors(output_dir, duration)

    # Launch ALL agents simultaneously (reproduces #15487)
    all_procs: list[list[subprocess.Popen]] = []
    agent_starts: list[float] = []
    for agent in agents:
        print(f"  [launch] {agent['name']}: {agent['task']}")
        procs = agent["start_fn"]()
        all_procs.append(procs)
        agent_starts.append(time.time())

    # Wait for duration
    print(f"  [wait] Stress running for {duration}s...")
    time.sleep(duration)

    # Kill all stress processes
    results: list[dict[str, Any]] = []
    for i, (agent, procs) in enumerate(zip(agents, all_procs)):
        elapsed = time.time() - agent_starts[i]
        # Check if any process exited early (failure/OOM)
        exit_codes = [p.poll() for p in procs]
        failed = any(c is not None and c != 0 for c in exit_codes)
        kill_procs(procs)

        result = {
            "name": agent["name"],
            "task": agent["task"],
            "duration_s": round(elapsed, 1),
            "exit_code": 1 if failed else 0,
            "mode": "blind",
        }
        status = "[err] FAILED" if failed else "[ok]"
        print(f"  {status} {agent['name']}: {elapsed:.1f}s")
        results.append(result)

    # Stop collectors
    stop_bg_collectors(metrics_proc, resp_proc)
    cleanup_all()

    # Save agent results
    with open(output_dir / "agent_results.json", "w") as f:
        json.dump(results, f, indent=2)

    return results


# ---------------------------------------------------------------------------
# Scenario B: Axon-Informed Agents
# ---------------------------------------------------------------------------

def run_scenario_b(agents: list[dict], duration: float, output_dir: Path, axon_bin: str) -> list[dict[str, Any]]:
    """Run agents sequentially with Axon MCP queries for intelligent scheduling."""
    output_dir.mkdir(parents=True, exist_ok=True)

    print("\n" + "=" * 60)
    print("  SCENARIO B: AXON-INFORMED AGENTS (hardware-aware)")
    print("=" * 60)

    # --- Start webhook collector ---
    webhook = WebhookCollector()
    webhook.start()

    # --- Create alert dispatch config ---
    cfg_dir = tempfile.mkdtemp(prefix="axon_demo_cfg_")
    cfg = {
        "channels": [{
            "type": "webhook",
            "id": "demo",
            "url": webhook.url,
            "filters": {"severity": [], "alert_types": ["*"]},
        }]
    }
    (Path(cfg_dir) / "alert-dispatch.json").write_text(json.dumps(cfg))

    data_dir = tempfile.mkdtemp(prefix="axon_demo_data_")

    # --- Start Axon server ---
    env = os.environ.copy()
    env["AXON_CONFIG_DIR"] = cfg_dir
    env["AXON_DATA_DIR"] = data_dir
    env["AXON_TEST_PREV_RAM_PRESSURE"] = "normal"
    env["AXON_TEST_PREV_IMPACT_LEVEL"] = "healthy"
    env["AXON_TEST_PREV_THROTTLING"] = "0"
    env["AXON_TEST_PRESERVE_PREV_DURING_WARMUP"] = "1"

    print(f"  [info] Starting Axon server: {axon_bin}")
    axon_proc = subprocess.Popen(
        [axon_bin, "serve"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True, bufsize=1, env=env,
    )

    # Wait for collector warm-up (3 ticks × ~2.5s)
    print("  [info] Waiting 8s for Axon collector warm-up...")
    time.sleep(8)

    # --- Initialize MCP client ---
    mcp = AxonMCPClient(axon_proc)
    if not mcp.initialize():
        print("  [err] MCP initialization failed", file=sys.stderr)
        axon_proc.terminate()
        webhook.stop()
        return []

    print("  [ok] MCP initialized")

    # --- Start background collectors ---
    total_est_duration = duration * len(agents) + 60  # rough estimate
    metrics_proc, resp_proc = start_bg_collectors(output_dir, total_est_duration)

    # --- Run each agent sequentially with Axon awareness ---
    results: list[dict[str, Any]] = []
    all_decisions: list[dict[str, Any]] = []

    for agent in agents:
        print(f"\n  --- {agent['name']}: {agent['task']} ---")

        runner = AgentWorkloadRunner(agent["name"], mcp, query_interval=30.0)

        # Pre-flight check: wait for adequate headroom
        decision = runner.wait_for_headroom(timeout_s=120.0, poll_s=5.0)

        # Start the workload
        print(f"  [launch] {agent['name']} (decision: {decision})")
        procs = agent["start_fn"]()
        agent_start = time.time()

        # Monitor during execution
        # We monitor the first process as representative
        if procs:
            runner.monitor_during(procs[0], max_duration=duration)

        # Wait for remaining duration if processes are still running
        elapsed = time.time() - agent_start
        remaining = duration - elapsed
        if remaining > 0:
            time.sleep(remaining)

        # Kill stress and assess
        kill_procs(procs)
        agent_end = time.time()

        runner.post_workload_assessment()
        cleanup_all()

        result = {
            "name": agent["name"],
            "task": agent["task"],
            "duration_s": round(agent_end - agent_start, 1),
            "exit_code": 0,
            "mode": "axon_informed",
            "decision": decision,
            "mcp_queries": len(runner.decisions),
        }
        print(f"  [ok] {agent['name']}: {result['duration_s']}s, {result['mcp_queries']} MCP queries")
        results.append(result)
        all_decisions.extend(runner.decisions)

        # Brief cooldown between agents
        time.sleep(5)

    # --- Final MCP queries (BEFORE shutting down Axon) ---
    print("\n  [info] Final session health + blame queries...")
    final_health = mcp.session_health()
    if final_health and final_health.get("ok"):
        data = final_health.get("data", {})
        print(f"  [info] Session: worst_impact={data.get('worst_impact_level', '?')}, "
              f"alerts={data.get('alert_count', 0)}")

    blame = mcp.process_blame()

    # --- Shutdown ---
    stop_bg_collectors(metrics_proc, resp_proc)
    axon_proc.terminate()
    try:
        axon_proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        axon_proc.kill()
    webhook.stop()

    # --- Save results ---
    with open(output_dir / "agent_results.json", "w") as f:
        json.dump(results, f, indent=2)

    with open(output_dir / "decisions.json", "w") as f:
        json.dump(all_decisions, f, indent=2)

    webhook.save(str(output_dir / "alerts.json"))

    if blame:
        with open(output_dir / "process_blame.json", "w") as f:
            json.dump(blame, f, indent=2, default=str)

    # Cleanup temp dirs
    shutil.rmtree(cfg_dir, ignore_errors=True)
    shutil.rmtree(data_dir, ignore_errors=True)

    return results


# ---------------------------------------------------------------------------
# Main
# ---------------------------------------------------------------------------

def main() -> int:
    ap = argparse.ArgumentParser(
        description="Claude + Axon: Parallel Agent Intelligence Demo",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  Quick test (30s, all agents):
    python3 scripts/comparative_stress_test.py --axon-bin ./target/release/axon --duration 30

  Full demo (120s):
    python3 scripts/comparative_stress_test.py --axon-bin ./target/release/axon --duration 120
""",
    )
    ap.add_argument("--axon-bin", required=True, help="Path to axon binary")
    ap.add_argument("--duration", type=float, default=120.0,
                    help="Duration per agent task in seconds (default: 120)")
    ap.add_argument("--output-dir", default="comparative_stress_test_results",
                    help="Output directory (default: comparative_stress_test_results)")
    args = ap.parse_args()

    axon_bin = args.axon_bin
    if not Path(axon_bin).is_file():
        print(f"[err] Axon binary not found: {axon_bin}", file=sys.stderr)
        print("  Run: cargo build --release -p axon-cli", file=sys.stderr)
        return 2

    output_dir = Path(args.output_dir)
    output_dir.mkdir(parents=True, exist_ok=True)
    scenario_a_dir = output_dir / "scenario_a_blind"
    scenario_b_dir = output_dir / "scenario_b_axon_informed"

    agents = get_agent_tasks(args.duration)

    print("=" * 60)
    print("  CLAUDE + AXON: PARALLEL AGENT INTELLIGENCE DEMO")
    print("=" * 60)
    print(f"  Agents: {len(agents)}")
    print(f"  Duration per agent: {args.duration}s")
    print(f"  Axon binary: {axon_bin}")
    print(f"  Output: {output_dir}")

    # --- Scenario A ---
    try:
        a_results = run_scenario_a(agents, args.duration, scenario_a_dir)
    except Exception as e:
        print(f"\n[err] Scenario A failed: {e}", file=sys.stderr)
        a_results = []
    finally:
        cleanup_all()

    # --- Cooldown ---
    print("\n[info] Cooldown: waiting 30s for system recovery...")
    time.sleep(30)

    # --- Scenario B ---
    try:
        b_results = run_scenario_b(agents, args.duration, scenario_b_dir, axon_bin)
    except Exception as e:
        print(f"\n[err] Scenario B failed: {e}", file=sys.stderr)
        b_results = []
    finally:
        cleanup_all()

    # --- Report ---
    print("\n" + "=" * 60)
    print("  GENERATING REPORT")
    print("=" * 60)

    generate_report(scenario_a_dir, scenario_b_dir, output_dir)

    # --- Summary ---
    a_failures = sum(1 for r in a_results if r.get("exit_code", 0) != 0)
    b_failures = sum(1 for r in b_results if r.get("exit_code", 0) != 0)

    print("\n" + "=" * 60)
    print("  SUMMARY")
    print("=" * 60)
    print(f"{'':>2}{'Agent':<20} {'Blind':>10} {'Axon':>10}")
    print("-" * 44)
    for i in range(len(agents)):
        a = a_results[i] if i < len(a_results) else {}
        b = b_results[i] if i < len(b_results) else {}
        name = a.get("name", b.get("name", f"Agent {i+1}"))
        a_s = f"{a.get('duration_s', '?')}s" if a else "N/A"
        b_s = f"{b.get('duration_s', '?')}s" if b else "N/A"
        a_ok = "ok" if a.get("exit_code", 1) == 0 else "FAIL"
        b_ok = "ok" if b.get("exit_code", 1) == 0 else "FAIL"
        print(f"  {name:<20} {a_s:>6} {a_ok:>3}  {b_s:>6} {b_ok:>3}")

    print(f"\n  Scenario A failures: {a_failures}/{len(a_results)}")
    print(f"  Scenario B failures: {b_failures}/{len(b_results)}")
    print(f"\n  Report: {output_dir / 'comparison_report.md'}")
    print(f"  Metrics: {output_dir / 'comparison_metrics.json'}")

    return 0 if b_failures == 0 else 1


if __name__ == "__main__":
    raise SystemExit(main())
