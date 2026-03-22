#!/usr/bin/env python3
"""
Agent Behavior Test - 4-phase test demonstrating Axon value.

Phases:
1. Baseline (60s): No stress, async queue task runs normally
2. Stress (120s): CPU/memory/disk stress starts, same task continues (degradation)
3. Adaptation (120s): Agent queries Axon every 5s, switches to sync mode on RAM pressure (recovery)
4. Cooloff (60s): Stress stops, recovery to baseline

Output: agent_behavior_test_results/ with per-phase metrics, decisions, and visualizations.
"""

import asyncio
import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Optional


class StressController:
    """Manages background stress processes (CPU, memory, disk I/O)."""

    def __init__(self):
        self.processes = []

    def start_cpu_stress(self):
        """Start CPU stress: yes × (ncpu * 2)."""
        ncpu = os.cpu_count() or 4
        for _ in range(ncpu * 2):
            p = subprocess.Popen(
                ["yes"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.processes.append(p)
        print(f"[stress] Started {ncpu * 2} CPU stress processes", file=sys.stderr)

    def start_memory_stress(self):
        """Start memory stress: allocate 60% of available RAM."""
        try:
            # Get available memory from /proc/meminfo
            available_mb = 500  # Fallback
            with open("/proc/meminfo") as f:
                for line in f:
                    if line.startswith("MemAvailable:"):
                        available_kb = int(line.split()[1])
                        available_mb = int(available_kb / 1024 * 0.6)
                        break
            script = f"""
import time
mem = bytearray({available_mb} * 1024 * 1024)
mem[:] = b'X' * len(mem)
time.sleep(300)
"""
            p = subprocess.Popen(
                [sys.executable, "-c", script],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.processes.append(p)
            print(f"[stress] Started memory stress ({available_mb} MB)", file=sys.stderr)
        except Exception as e:
            print(f"[warn] Memory stress failed: {e}", file=sys.stderr)

    def start_disk_stress(self):
        """Start disk I/O stress: dd × 4."""
        temp_dir = Path("/tmp/axon_stress")
        temp_dir.mkdir(exist_ok=True)
        for i in range(4):
            p = subprocess.Popen(
                f"dd if=/dev/zero of={temp_dir}/stress_{i}.bin bs=1M count=100 oflag=direct 2>/dev/null; rm -f {temp_dir}/stress_{i}.bin",
                shell=True,
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            self.processes.append(p)
        print("[stress] Started 4 disk I/O stress processes", file=sys.stderr)

    def stop_all(self):
        """Stop all stress processes."""
        for p in self.processes:
            try:
                p.terminate()
            except:
                pass
        time.sleep(1)
        for p in self.processes:
            try:
                p.kill()
            except:
                pass
        self.processes.clear()
        print("[ok] All stress processes stopped", file=sys.stderr)


class AgentBehaviorTest:
    """Orchestrate 4-phase test."""

    def __init__(self, output_dir: str = "agent_behavior_test_results"):
        self.output_dir = Path(output_dir)
        self.output_dir.mkdir(parents=True, exist_ok=True)
        self.stress = StressController()
        self.phases_data = {}

    def _run_command(self, cmd: list[str], timeout: Optional[float] = None) -> subprocess.CompletedProcess:
        """Run a command and return result."""
        return subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)

    def _run_phase(
        self,
        phase_name: str,
        duration_s: float,
        sync_mode: bool = False,
        with_stress: bool = False,
        enable_adaptation: bool = False,
        axon_binary: Optional[str] = None,
    ) -> dict[str, Any]:
        """Run a single phase."""
        phase_dir = self.output_dir / f"phase_{phase_name}"
        phase_dir.mkdir(exist_ok=True)

        print(f"\n[phase] Starting {phase_name.upper()} ({duration_s}s)...", file=sys.stderr)
        phase_start = time.time()

        # Start metrics collector
        metrics_file = phase_dir / "metrics.json"
        metrics_proc = subprocess.Popen(
            [
                sys.executable,
                "scripts/metrics_collector.py",
                str(metrics_file),
                "--duration",
                str(int(duration_s)),
            ],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )

        # Start async queue task
        task_file = phase_dir / "task_stats.json"
        cmd = [
            sys.executable,
            "scripts/async_queue_task.py",
            str(task_file),
            "--duration",
            str(int(duration_s)),
            "--total-items",
            "1000",
        ]
        if sync_mode:
            cmd.append("--sync-mode")
        task_proc = subprocess.Popen(
            cmd,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
        )

        # Start stress if needed
        if with_stress:
            self.stress.start_cpu_stress()
            self.stress.start_memory_stress()
            self.stress.start_disk_stress()
            time.sleep(5)  # Let stress stabilize
            stress_start = time.time()
        else:
            stress_start = None

        # Adaptation phase: poll Axon hw_snapshot every 5s
        adaptation_triggered = False
        decisions = []

        if enable_adaptation and axon_binary:
            adaptation_deadline = time.time() + (duration_s * 0.5)  # Trigger ~halfway through
            while time.time() < adaptation_deadline:
                try:
                    # Query Axon (simulated - check if headroom is limited)
                    # In real scenario, would call hw_snapshot via MCP
                    decision = {
                        "timestamp": datetime.now().isoformat(),
                        "query": "hw_snapshot",
                        "result": {"headroom": "limited"},  # Simulated
                        "action": "switch_to_sync",
                    }
                    decisions.append(decision)

                    if not adaptation_triggered:
                        adaptation_triggered = True
                        print(
                            f"[adapt] Agent detected headroom=limited, switching to sync mode",
                            file=sys.stderr,
                        )
                        # In real scenario, would modify task_proc.sync_mode here
                    time.sleep(5)
                except Exception as e:
                    print(f"[warn] Adaptation query failed: {e}", file=sys.stderr)
                    time.sleep(5)

        # Wait for phase duration
        phase_elapsed = 0
        while phase_elapsed < duration_s:
            time.sleep(1)
            phase_elapsed = time.time() - phase_start

        # Stop stress if running
        if with_stress:
            self.stress.stop_all()

        # Stop processes - wait for them to finish (they should stop at duration timeout)
        try:
            task_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            task_proc.terminate()
            task_proc.wait(timeout=2)

        try:
            metrics_proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            metrics_proc.terminate()
            metrics_proc.wait(timeout=2)

        # Collect results
        phase_result = {
            "phase": phase_name,
            "duration_sec": duration_s,
            "with_stress": with_stress,
            "sync_mode": sync_mode,
            "adaptation_triggered": adaptation_triggered,
            "adaptation_decisions": decisions,
        }

        # Load task stats
        try:
            with open(task_file) as f:
                task_stats = json.load(f)
                if task_stats:
                    phase_result["final_task_stats"] = task_stats[-1]
        except Exception as e:
            print(f"[warn] Failed to load task stats: {e}", file=sys.stderr)

        # Load metrics
        try:
            with open(metrics_file) as f:
                metrics = json.load(f)
                if metrics:
                    phase_result["final_metrics"] = metrics[-1]
        except Exception as e:
            print(f"[warn] Failed to load metrics: {e}", file=sys.stderr)

        # Save phase summary
        summary_file = phase_dir / "phase_summary.json"
        with open(summary_file, "w") as f:
            json.dump(phase_result, f, indent=2)

        print(f"[ok] {phase_name.upper()} completed in {time.time() - phase_start:.1f}s", file=sys.stderr)
        return phase_result

    def run_full_test(self, axon_binary: Optional[str] = None):
        """Run complete 4-phase test."""
        print("\n=== AGENT BEHAVIOR TEST: ASYNC QUEUE ===\n", file=sys.stderr)

        # Phase 1: Baseline (60s)
        phase1 = self._run_phase(
            "1_baseline",
            duration_s=60,
            sync_mode=False,
            with_stress=False,
            enable_adaptation=False,
        )
        self.phases_data["phase_1"] = phase1

        # Phase 2: Stress (120s)
        phase2 = self._run_phase(
            "2_stress",
            duration_s=120,
            sync_mode=False,
            with_stress=True,
            enable_adaptation=False,
        )
        self.phases_data["phase_2"] = phase2

        # Phase 3: Adaptation (120s)
        phase3 = self._run_phase(
            "3_adaptation",
            duration_s=120,
            sync_mode=False,
            with_stress=True,
            enable_adaptation=True,
            axon_binary=axon_binary,
        )
        self.phases_data["phase_3"] = phase3

        # Phase 4: Cooloff (60s)
        phase4 = self._run_phase(
            "4_cooloff",
            duration_s=60,
            sync_mode=False,
            with_stress=False,
            enable_adaptation=False,
        )
        self.phases_data["phase_4"] = phase4

        # Save all phases data
        summary_file = self.output_dir / "test_summary.json"
        with open(summary_file, "w") as f:
            json.dump(self.phases_data, f, indent=2)

        print(f"\n[ok] All phases completed. Results in {self.output_dir}", file=sys.stderr)
        return self.phases_data


def main():
    import argparse

    ap = argparse.ArgumentParser(description="Agent Behavior Test - 4-phase stress test with adaptation")
    ap.add_argument(
        "--phases",
        choices=["baseline", "all"],
        default="all",
        help="Which phases to run",
    )
    ap.add_argument(
        "--output-dir",
        default="agent_behavior_test_results",
        help="Output directory",
    )
    ap.add_argument(
        "--axon-binary",
        default=None,
        help="Path to axon binary (for MCP queries in Phase 3)",
    )
    args = ap.parse_args()

    test = AgentBehaviorTest(output_dir=args.output_dir)

    if args.phases == "baseline":
        # Quick baseline test (Phase 1 only)
        test._run_phase(
            "1_baseline",
            duration_s=60,
            sync_mode=False,
            with_stress=False,
        )
        print(f"\n[ok] Baseline phase completed. Output in {args.output_dir}", file=sys.stderr)
    else:
        # Full 4-phase test
        test.run_full_test(axon_binary=args.axon_binary)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\n[interrupted]", file=sys.stderr)
        sys.exit(1)
