#!/usr/bin/env python3
"""Generic agent-platform live demo for Axon.

The point of this demo is not to show hardware metrics. It shows that an
AI app-builder agent can ask Axon before spending more worker time/credits,
then choose safer behavior: continue lightweight work, cap parallelism, avoid
extra tool fan-out, and flag UI/runtime pressure.
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


def group_count(groups: list[str], name: str) -> int:
    prefix = f"{name} x"
    for group in groups:
        if group.startswith(prefix):
            return int(group[len(prefix) :])
    return 0


def spawn_tool_workers(count: int) -> tuple[tempfile.TemporaryDirectory[str], list[subprocess.Popen[str]]]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required for this demo")
    tmp = tempfile.TemporaryDirectory(prefix="axon-agent-platform-tool-")
    script = Path(tmp.name) / "playwright-mcp-worker.js"
    script.write_text("setInterval(() => {}, 1000);\n", encoding="utf-8")
    children = [
        subprocess.Popen(
            [node, str(script), "--axon-agent-platform-tool-worker"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        for _ in range(count)
    ]
    return tmp, children


def spawn_renderer_spin(seconds: int = 20) -> subprocess.Popen[str]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required for this demo")
    script = (
        f"const end = Date.now() + {seconds * 1000}; "
        "while (Date.now() < end) { Math.sqrt(Math.random()); }"
    )
    return subprocess.Popen(
        [
            node,
            "-e",
            script,
            "/Applications/Codex.app/Contents/Frameworks/Codex Helper (Renderer).app",
            "--type=renderer",
            "--axon-agent-platform-renderer-pressure",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
        text=True,
    )


def cleanup(children: list[subprocess.Popen[str]], tmp: tempfile.TemporaryDirectory[str] | None = None) -> None:
    for child in children:
        if child.poll() is None:
            child.terminate()
    deadline = time.time() + 4
    for child in children:
        try:
            child.wait(timeout=max(0.1, deadline - time.time()))
        except subprocess.TimeoutExpired:
            child.kill()
    if tmp is not None:
        tmp.cleanup()


def run_parallel_workers(count: int) -> int:
    with tempfile.TemporaryDirectory(prefix="axon-agent-platform-parallel-") as tmp:
        children = [
            subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    "import time; time.sleep(0.6)",
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            for _ in range(count)
        ]
        max_seen = 0
        while any(child.poll() is None for child in children):
            max_seen = max(max_seen, sum(1 for child in children if child.poll() is None))
            time.sleep(0.05)
        for child in children:
            child.wait(timeout=2)
        return max_seen


def assert_no_leftovers() -> None:
    proc = subprocess.run(
        [
            "pgrep",
            "-fl",
            "axon-agent-platform-tool-worker|axon-agent-platform-renderer-pressure|playwright-mcp-worker",
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    lines = [
        line
        for line in proc.stdout.splitlines()
        if "demo_agent_platform_live.py" not in line and "pgrep" not in line
    ]
    if lines:
        raise AssertionError("leftover demo processes:\n" + "\n".join(lines))


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    print("AXON AGENT PLATFORM LIVE DEMO")
    print("Scenario: an AI app-builder worker is about to spend more time/credits on tool-heavy build/debug work.")
    print()

    baseline = run_axon(axon_bin, "agent_runtime_health")["data"]
    workload = run_axon(axon_bin, "workload_advice")["data"]
    print(
        f"[baseline] runtime={baseline['process_count']} agent processes, "
        f"mcp={baseline['mcp_server_count']}, stale_mcp={baseline['stale_mcp_server_count']}, "
        f"recommendation={workload['recommendation']}, safe_parallelism={workload.get('safe_parallelism')}"
    )

    before_mcp = baseline["mcp_server_count"]
    before_playwright = group_count(baseline["duplicate_mcp_server_groups"], "playwright-mcp")
    tmp, children = spawn_tool_workers(4)
    try:
        time.sleep(1)
        no_policy = run_axon(axon_bin, "agent_runtime_health")["data"]
    finally:
        cleanup(children, tmp)
    time.sleep(0.5)
    after_cleanup = run_axon(axon_bin, "agent_runtime_health")["data"]

    no_policy_mcp = no_policy["mcp_server_count"]
    no_policy_playwright = group_count(no_policy["duplicate_mcp_server_groups"], "playwright-mcp")
    print(
        f"[without Axon policy] spawned 4 extra tool workers; "
        f"MCP {before_mcp}->{no_policy_mcp}; playwright group {before_playwright}->{no_policy_playwright}"
    )
    print(f"[cleanup proof] temporary workers cleaned; MCP returned to {after_cleanup['mcp_server_count']}")

    policy_runtime = run_axon(axon_bin, "agent_runtime_health")["data"]
    requested_tool_spawns = 4
    allow_tool_spawns = (
        not policy_runtime["duplicate_mcp_server_groups"]
        and policy_runtime["stale_mcp_server_count"] < 8
        and workload["recommendation"] not in {"defer", "cooldown"}
    )
    actual_tool_spawns = requested_tool_spawns if allow_tool_spawns else 0
    avoided_tool_spawns = requested_tool_spawns - actual_tool_spawns
    print(
        f"[with Axon policy] requested {requested_tool_spawns} new tool workers; "
        f"spawned {actual_tool_spawns}; avoided {avoided_tool_spawns}"
    )

    requested_parallelism = 4
    safe_parallelism = int(workload.get("safe_parallelism") or requested_parallelism)
    safe_parallelism = max(1, min(requested_parallelism, safe_parallelism))
    no_policy_concurrency = run_parallel_workers(requested_parallelism)
    policy_concurrency = run_parallel_workers(safe_parallelism)
    print(
        f"[parallel build/debug loop] without Axon concurrency={no_policy_concurrency}; "
        f"with Axon concurrency={policy_concurrency}; capped {requested_parallelism}->{safe_parallelism}"
    )

    before_ui = run_axon(axon_bin, "agent_runtime_health")["data"]
    spinner = spawn_renderer_spin()
    try:
        time.sleep(1)
        during_ui = run_axon(axon_bin, "agent_runtime_health")["data"]
    finally:
        cleanup([spinner])
    print(
        f"[runtime UX guard] renderer CPU {before_ui['renderer_cpu_pct']:.0f}%->{during_ui['renderer_cpu_pct']:.0f}%; "
        f"high_cpu_ui={during_ui['high_cpu_ui_process_count']}"
    )

    impacts = policy_runtime.get("workflow_impacts", [])
    if impacts:
        print("[agent-facing impacts]")
        for impact in impacts[:3]:
            print(f"- {impact['use_case']}: {impact['business_impact']}")
            print(f"  action: {impact['recommended_action']}")

    assert_no_leftovers()
    if avoided_tool_spawns <= 0:
        raise AssertionError("demo did not prove avoided tool spawns")
    if policy_concurrency > safe_parallelism:
        raise AssertionError("policy exceeded safe parallelism")

    print()
    print("PITCH TAKEAWAY")
    print(
        "Axon lets an app-building agent continue safe work while avoiding wasted credits: "
        f"avoided {avoided_tool_spawns} extra tool workers, capped parallelism "
        f"{requested_parallelism}->{safe_parallelism}, and detected runtime UI pressure."
    )
    print("[ok] Agent platform live demo passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
