#!/usr/bin/env python3
"""Live proof: Axon enables useful agent work, not a blanket stop sign.

The policy in this script deliberately distinguishes between:
- safe lightweight local work that should still run on a pressured machine,
- heavy parallel work that should be reduced to Axon's safe parallelism,
- MCP-heavy fan-out that should reuse existing runtime/tool state instead of
  spawning more helper processes.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def run_axon(axon_bin: str, tool: str) -> dict:
    proc = subprocess.run(
        [axon_bin, "query", tool],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=35,
        check=True,
    )
    return json.loads(proc.stdout)


def allow_lightweight_local_task(runtime: dict, workload: dict) -> tuple[bool, str]:
    """Allow read-only local checks even when heavy work is deferred."""
    data = runtime["data"]
    advice = workload["data"]
    if data.get("high_cpu_ui_process_count", 0) > 0 and data.get("total_cpu_pct", 0.0) > 80.0:
        return False, "UI and runtime CPU are both hot; wait before even lightweight checks."
    if advice.get("risk") == "critical":
        return True, "Critical risk blocks heavy fan-out, but read-only local inspection is still useful."
    return True, "Axon does not block lightweight read-only local inspection."


def planned_parallelism(workload: dict, requested: int) -> tuple[int, str]:
    advice = workload["data"]
    safe = advice.get("safe_parallelism")
    if safe is None:
        return requested, "Axon did not cap parallelism."
    capped = max(1, min(requested, int(safe)))
    if capped < requested:
        return capped, f"Axon capped parallelism from {requested} to {capped}."
    return capped, "Requested parallelism is within Axon's safe limit."


def allow_new_mcp_spawn(runtime: dict) -> tuple[bool, str]:
    data = runtime["data"]
    if data.get("duplicate_mcp_server_groups"):
        return False, "Reuse existing MCP/tool stack; duplicate MCP groups are already present."
    if data.get("stale_mcp_server_count", 0) >= 8:
        return False, "Clean stale MCP servers before opening more MCP-heavy tools."
    return True, "MCP runtime looks clean enough to spawn another tool."


def run_lightweight_repo_check() -> None:
    proc = subprocess.run(
        ["git", "status", "--short"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=10,
        check=True,
    )
    print(f"[task lightweight] git status returned {len(proc.stdout.splitlines())} changed entries")


def run_capped_parallel_work(worker_count: int) -> int:
    """Run a tiny CPU-light parallel workload and return the max concurrency used."""
    with tempfile.TemporaryDirectory(prefix="axon-policy-workers-") as tmp:
        out_dir = Path(tmp)
        children: list[subprocess.Popen[str]] = []
        for idx in range(worker_count):
            children.append(
                subprocess.Popen(
                    [
                        sys.executable,
                        "-c",
                        (
                            "import pathlib, sys, time; "
                            "p=pathlib.Path(sys.argv[1]); "
                            "p.write_text('started'); "
                            "time.sleep(0.5); "
                            "p.write_text('done')"
                        ),
                        str(out_dir / f"worker-{idx}.txt"),
                    ],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    text=True,
                )
            )
        max_seen = 0
        while any(child.poll() is None for child in children):
            active = sum(1 for child in children if child.poll() is None)
            max_seen = max(max_seen, active)
            time.sleep(0.05)
        for child in children:
            child.wait(timeout=2)
        return max_seen


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    runtime = run_axon(axon_bin, "agent_runtime_health")
    workload = run_axon(axon_bin, "workload_advice")

    allow_light, light_reason = allow_lightweight_local_task(runtime, workload)
    print(f"[policy lightweight] decision={'run' if allow_light else 'wait'} reason={light_reason}")
    if not allow_light:
        raise SystemExit("[err] this scenario expected lightweight work to remain allowed")
    run_lightweight_repo_check()

    requested_parallelism = 4
    capped_parallelism, cap_reason = planned_parallelism(workload, requested_parallelism)
    print(
        f"[policy parallel] requested={requested_parallelism} actual={capped_parallelism} reason={cap_reason}"
    )
    max_seen = run_capped_parallel_work(capped_parallelism)
    print(f"[task parallel] max_concurrency_seen={max_seen}")
    if max_seen > capped_parallelism:
        raise SystemExit("[err] agent exceeded Axon's safe parallelism")

    allow_mcp, mcp_reason = allow_new_mcp_spawn(runtime)
    print(f"[policy mcp] decision={'spawn_new_mcp' if allow_mcp else 'reuse_existing_or_cleanup'} reason={mcp_reason}")
    before_mcp = runtime["data"]["mcp_server_count"]
    after_mcp = run_axon(axon_bin, "agent_runtime_health")["data"]["mcp_server_count"]
    if not allow_mcp and after_mcp > before_mcp:
        raise SystemExit("[err] MCP count increased even though policy denied MCP spawn")

    print("[ok] Axon policy allowed useful work while blocking only risky fan-out")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
