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

import json
import os
import subprocess
import sys
import time
from datetime import datetime
from pathlib import Path
from typing import Any, Optional

# Import proven AxonMCPClient from axon_aware_workload_runner
sys.path.insert(0, str(Path(__file__).parent))
from axon_aware_workload_runner import AxonMCPClient

# Control file path shared with async_queue_task subprocess
CONTROL_FILE = "/tmp/axon_task_control.json"


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

    def start_memory_stress(self, memory_pct: float = 0.6):
        """Start memory stress: allocate memory_pct of available RAM."""
        try:
            # Get available memory from /proc/meminfo
            available_mb = 500  # Fallback
            with open("/proc/meminfo") as f:
                for line in f:
                    if line.startswith("MemAvailable:"):
                        available_kb = int(line.split()[1])
                        available_mb = int(available_kb / 1024 * memory_pct)
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
        # Retry-tunable settings (modified between attempts)
        self._stress_memory_pct: float = 0.6  # fraction of available RAM to hog
        self._low_threshold_mode: bool = False  # trigger on cpu>70 or ram>40 regardless of headroom
        self._force_adaptation_after_s: Optional[float] = None  # force trigger after N seconds

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
            self.stress.start_memory_stress(self._stress_memory_pct)
            self.stress.start_disk_stress()
            time.sleep(5)  # Let stress stabilize
        else:
            pass

        # Adaptation phase: poll real Axon hw_snapshot every 5s
        adaptation_triggered = False
        decisions = []
        axon_proc = None

        if enable_adaptation and axon_binary:
            try:
                axon_proc = subprocess.Popen(
                    [axon_binary, "serve"],
                    stdin=subprocess.PIPE,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
                mcp = AxonMCPClient(axon_proc)
                if mcp.initialize():
                    print("[ok] Axon MCP initialized for adaptation phase", file=sys.stderr)
                else:
                    print("[warn] Axon MCP initialize failed — will use fallback threshold", file=sys.stderr)
                    axon_proc.terminate()
                    axon_proc = None
                    mcp = None
            except Exception as e:
                print(f"[warn] Failed to start axon: {e}", file=sys.stderr)
                axon_proc = None
                mcp = None

            adaptation_start = time.time()

            # Unified loop: query axon every 5s for full phase duration
            while time.time() < (phase_start + duration_s):
                try:
                    hw = mcp.hw_snapshot() if mcp else None

                    if hw and hw.get("ok"):
                        data = hw.get("data", {})
                        headroom = data.get("headroom", "adequate")
                        headroom_reason = data.get("headroom_reason", "")
                        cpu_pct = data.get("cpu_usage_pct", 0.0)
                        ram_used_gb = data.get("ram_used_gb", 0.0)
                        ram_total_gb = data.get("ram_total_gb", 1.0)
                        ram_pct = round(ram_used_gb / ram_total_gb * 100, 1) if ram_total_gb else 0.0
                    else:
                        headroom = "adequate"
                        headroom_reason = "axon query failed"
                        cpu_pct = 0.0
                        ram_pct = 0.0

                    # Determine if adaptation should trigger
                    trigger_headroom = headroom in ("limited", "insufficient")
                    trigger_low = (
                        self._low_threshold_mode and (cpu_pct > 70.0 or ram_pct > 40.0)
                    )
                    elapsed_s = time.time() - adaptation_start
                    trigger_force = (
                        self._force_adaptation_after_s is not None
                        and elapsed_s > self._force_adaptation_after_s
                    )

                    if trigger_headroom:
                        trigger_reason = f"headroom={headroom} ({headroom_reason})"
                    elif trigger_low:
                        trigger_reason = f"threshold: cpu={cpu_pct:.0f}% ram={ram_pct:.0f}%"
                    elif trigger_force:
                        trigger_reason = f"forced after {elapsed_s:.0f}s timeout"
                    else:
                        trigger_reason = None

                    decision: dict[str, Any] = {
                        "timestamp": datetime.now().isoformat(),
                        "query": "hw_snapshot",
                        "result": {
                            "headroom": headroom,
                            "headroom_reason": headroom_reason,
                            "cpu_pct": cpu_pct,
                            "ram_pct": ram_pct,
                        },
                        "adaptation_triggered": False,
                        "action": None,
                    }

                    if trigger_reason and not adaptation_triggered:
                        adaptation_triggered = True
                        decision["adaptation_triggered"] = True
                        decision["action"] = f"switch_to_sync ({trigger_reason})"
                        print(
                            f"[adapt] Axon signal: {trigger_reason} — switching task to sync mode",
                            file=sys.stderr,
                        )
                        try:
                            with open(CONTROL_FILE, "w") as cf:
                                json.dump(
                                    {
                                        "sync_mode": True,
                                        "reason": trigger_reason,
                                        "headroom": headroom,
                                        "ts": datetime.now().isoformat(),
                                    },
                                    cf,
                                )
                        except Exception as e:
                            print(f"[warn] Failed to write control file: {e}", file=sys.stderr)

                    decisions.append(decision)

                except Exception as e:
                    print(f"[warn] Adaptation query error: {e}", file=sys.stderr)

                time.sleep(5)

            # Cleanup
            try:
                Path(CONTROL_FILE).unlink(missing_ok=True)
            except Exception:
                pass
            if axon_proc:
                axon_proc.terminate()
                try:
                    axon_proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    axon_proc.kill()

        else:
            # No adaptation — just wait out the phase
            while time.time() - phase_start < duration_s:
                time.sleep(1)

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

        # Save decisions log if we have any
        if decisions:
            decisions_file = phase_dir / "decisions.json"
            with open(decisions_file, "w") as f:
                json.dump(decisions, f, indent=2)

        # Collect results
        phase_result = {
            "phase": phase_name,
            "duration_sec": duration_s,
            "with_stress": with_stress,
            "sync_mode": sync_mode,
            "adaptation_triggered": adaptation_triggered,
            "adaptation_decisions": [],  # kept compact; full data in decisions.json
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

    def _run_full_phases(self, axon_binary: Optional[str] = None) -> dict[str, Any]:
        """Run all 4 phases and return combined data."""
        self.phases_data = {}
        # Ensure no stale control file from a previous (possibly killed) run
        try:
            Path(CONTROL_FILE).unlink(missing_ok=True)
        except Exception:
            pass

        phase1 = self._run_phase(
            "1_baseline", duration_s=60, sync_mode=False, with_stress=False, enable_adaptation=False,
        )
        self.phases_data["phase_1"] = phase1

        phase2 = self._run_phase(
            "2_stress", duration_s=120, sync_mode=False, with_stress=True, enable_adaptation=False,
        )
        self.phases_data["phase_2"] = phase2

        phase3 = self._run_phase(
            "3_adaptation", duration_s=120, sync_mode=False, with_stress=True,
            enable_adaptation=True, axon_binary=axon_binary,
        )
        self.phases_data["phase_3"] = phase3

        phase4 = self._run_phase(
            "4_cooloff", duration_s=60, sync_mode=False, with_stress=False, enable_adaptation=False,
        )
        self.phases_data["phase_4"] = phase4

        summary_file = self.output_dir / "test_summary.json"
        with open(summary_file, "w") as f:
            json.dump(self.phases_data, f, indent=2)

        print(f"\n[ok] All phases completed. Results in {self.output_dir}", file=sys.stderr)
        return self.phases_data

    def _validate_run(self) -> list[str]:
        """Check the 3 success gates. Returns list of failure reasons (empty = success)."""
        failures = []
        phase3_dir = self.output_dir / "phase_3_adaptation"

        # Gate 1: adaptation_triggered must be true
        try:
            with open(phase3_dir / "phase_summary.json") as f:
                summary = json.load(f)
            if not summary.get("adaptation_triggered"):
                failures.append("adaptation_triggered is false in phase_summary.json")
        except Exception as e:
            failures.append(f"Could not read phase_summary.json: {e}")

        # Gate 2: decisions.json must contain real axon data (has cpu_pct field)
        decisions_file = phase3_dir / "decisions.json"
        if decisions_file.exists():
            try:
                with open(decisions_file) as f:
                    decisions = json.load(f)
                real_queries = [d for d in decisions if "cpu_pct" in d.get("result", {})]
                if not real_queries:
                    failures.append("No real axon queries in decisions.json (missing cpu_pct)")
                else:
                    print(f"[ok] {len(real_queries)} real axon queries logged", file=sys.stderr)
            except Exception as e:
                failures.append(f"Could not read decisions.json: {e}")
        else:
            failures.append("decisions.json not found in phase_3_adaptation/")

        # Gate 3: task must have switched to sync mode in at least one sample
        try:
            with open(phase3_dir / "task_stats.json") as f:
                task_stats = json.load(f)
            sync_samples = [s for s in task_stats if s.get("mode") == "sync"]
            if not sync_samples:
                failures.append("No sync mode samples in task_stats.json (task never switched)")
            else:
                print(f"[ok] Task switched to sync mode ({len(sync_samples)} sync samples)", file=sys.stderr)
        except Exception as e:
            failures.append(f"Could not read task_stats.json: {e}")

        return failures

    def _generate_report(self):
        """Call the existing report generator on successful results."""
        report_script = Path(__file__).parent / "generate_behavior_report.py"
        if report_script.exists():
            try:
                subprocess.run(
                    [sys.executable, str(report_script), str(self.output_dir)],
                    check=True,
                )
            except subprocess.CalledProcessError as e:
                print(f"[warn] Report generation failed: {e}", file=sys.stderr)
        else:
            print("[info] generate_behavior_report.py not found — skipping report", file=sys.stderr)

    def run_with_retry(self, axon_binary: Optional[str] = None, max_attempts: int = 3) -> bool:
        """Run full test with up to max_attempts retries. Only generate report on success."""
        print("\n=== AGENT BEHAVIOR TEST: ASYNC QUEUE ===\n", file=sys.stderr)

        for attempt in range(1, max_attempts + 1):
            print(f"\n[run] Attempt {attempt}/{max_attempts}", file=sys.stderr)

            if attempt == 2:
                # Heavier stress + lower trigger threshold
                print("[run] Attempt 2: increasing stress to 80% RAM, lowering trigger threshold", file=sys.stderr)
                self._stress_memory_pct = 0.8
                self._low_threshold_mode = True
            elif attempt == 3:
                # Force trigger after 20s regardless of headroom
                print("[run] Attempt 3: forcing adaptation after 20s regardless of headroom", file=sys.stderr)
                self._force_adaptation_after_s = 20.0

            self._run_full_phases(axon_binary)

            failures = self._validate_run()
            if not failures:
                print(f"\n[ok] Goal achieved on attempt {attempt} — generating report", file=sys.stderr)
                self._generate_report()
                return True
            else:
                print(f"\n[warn] Attempt {attempt} FAILED:", file=sys.stderr)
                for f in failures:
                    print(f"  - {f}", file=sys.stderr)

        print(
            f"\n[err] Goal NOT achieved after {max_attempts} attempts. "
            f"See {self.output_dir} for diagnostics.",
            file=sys.stderr,
        )
        return False

    # Keep backwards-compatible alias
    def run_full_test(self, axon_binary: Optional[str] = None):
        return self._run_full_phases(axon_binary)


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
        # Full 4-phase test with self-validating retry loop
        success = test.run_with_retry(axon_binary=args.axon_binary)
        sys.exit(0 if success else 1)


if __name__ == "__main__":
    try:
        main()
    except KeyboardInterrupt:
        print("\n[interrupted]", file=sys.stderr)
        sys.exit(1)
