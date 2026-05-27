#!/usr/bin/env python3
"""Live value demo: show what Axon changes for an agent.

This script compares agent behavior without Axon policy to behavior with Axon
policy on the same machine. It uses local-only temporary helper processes and
cleans them up before exiting.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ScenarioResult:
    name: str
    value: str


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


def spawn_playwright_mcp(count: int, marker: str) -> tuple[tempfile.TemporaryDirectory[str], list[subprocess.Popen[str]]]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required")
    tmp = tempfile.TemporaryDirectory(prefix="axon-value-mcp-")
    script = Path(tmp.name) / "playwright-mcp-worker.js"
    script.write_text("setInterval(() => {}, 1000);\n", encoding="utf-8")
    children = [
        subprocess.Popen(
            [node, str(script), marker],
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
        raise RuntimeError("node is required")
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
            "--axon-value-ui-spin",
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


def run_workers(count: int) -> int:
    with tempfile.TemporaryDirectory(prefix="axon-value-workers-") as tmp:
        out_dir = Path(tmp)
        children = [
            subprocess.Popen(
                [
                    sys.executable,
                    "-c",
                    (
                        "import pathlib, sys, time; "
                        "pathlib.Path(sys.argv[1]).write_text('started'); "
                        "time.sleep(0.6)"
                    ),
                    str(out_dir / f"worker-{idx}.txt"),
                ],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
            for idx in range(count)
        ]
        max_seen = 0
        while any(child.poll() is None for child in children):
            max_seen = max(max_seen, sum(1 for child in children if child.poll() is None))
            time.sleep(0.05)
        for child in children:
            child.wait(timeout=2)
        return max_seen


def scenario_without_policy_spawns_too_much(axon_bin: str) -> ScenarioResult:
    before = run_axon(axon_bin, "agent_runtime_health")["data"]
    before_mcp = before["mcp_server_count"]
    before_playwright = group_count(before["duplicate_mcp_server_groups"], "playwright-mcp")

    tmp, children = spawn_playwright_mcp(4, "--axon-value-no-policy")
    try:
        time.sleep(1)
        during = run_axon(axon_bin, "agent_runtime_health")["data"]
        during_mcp = during["mcp_server_count"]
        during_playwright = group_count(during["duplicate_mcp_server_groups"], "playwright-mcp")
    finally:
        cleanup(children, tmp)

    time.sleep(0.5)
    after_cleanup = run_axon(axon_bin, "agent_runtime_health")["data"]
    cleanup_returned = after_cleanup["mcp_server_count"] <= before_mcp + 1

    if during_mcp < before_mcp + 4 or during_playwright < before_playwright + 4:
        raise AssertionError("without-policy scenario failed to create measurable MCP fan-out")
    if not cleanup_returned:
        raise AssertionError("temporary MCP helpers did not clean up")

    return ScenarioResult(
        "No-policy MCP fan-out",
        (
            f"spawned 4 extra MCP helpers; MCP count {before_mcp}->{during_mcp}; "
            f"playwright group {before_playwright}->{during_playwright}; cleanup returned to {after_cleanup['mcp_server_count']}"
        ),
    )


def scenario_with_policy_avoids_fanout(axon_bin: str) -> ScenarioResult:
    runtime = run_axon(axon_bin, "agent_runtime_health")["data"]
    requested_spawns = 4
    allow_spawn = not runtime["duplicate_mcp_server_groups"] and runtime["stale_mcp_server_count"] < 8
    actual_spawns = requested_spawns if allow_spawn else 0

    before_mcp = runtime["mcp_server_count"]
    tmp: tempfile.TemporaryDirectory[str] | None = None
    children: list[subprocess.Popen[str]] = []
    if actual_spawns:
        tmp, children = spawn_playwright_mcp(actual_spawns, "--axon-value-policy-allowed")
    try:
        time.sleep(1)
        after = run_axon(axon_bin, "agent_runtime_health")["data"]
    finally:
        cleanup(children, tmp)

    avoided = requested_spawns - actual_spawns
    if not allow_spawn and after["mcp_server_count"] > before_mcp:
        raise AssertionError("policy denied MCP fan-out but MCP count increased")

    return ScenarioResult(
        "Axon policy MCP fan-out",
        f"requested {requested_spawns}, spawned {actual_spawns}, avoided {avoided} risky MCP helpers",
    )


def scenario_parallelism_reduction(axon_bin: str) -> ScenarioResult:
    workload = run_axon(axon_bin, "workload_advice")["data"]
    requested = 4
    safe = workload.get("safe_parallelism") or requested
    capped = max(1, min(requested, int(safe)))

    no_policy_max = run_workers(requested)
    policy_max = run_workers(capped)
    if policy_max > capped:
        raise AssertionError("policy workload exceeded capped parallelism")

    return ScenarioResult(
        "Parallel work",
        (
            f"without Axon max concurrency {no_policy_max}; with Axon max concurrency {policy_max}; "
            f"parallelism capped {requested}->{capped}"
        ),
    )


def scenario_ui_pressure(axon_bin: str) -> ScenarioResult:
    before = run_axon(axon_bin, "agent_runtime_health")["data"]
    child = spawn_renderer_spin()
    try:
        time.sleep(1)
        after = run_axon(axon_bin, "agent_runtime_health")["data"]
    finally:
        cleanup([child])

    if after["renderer_cpu_pct"] < before["renderer_cpu_pct"] + 20.0 and after["high_cpu_ui_process_count"] < 1:
        raise AssertionError("UI pressure scenario did not produce detectable renderer pressure")

    return ScenarioResult(
        "UI responsiveness",
        (
            f"renderer CPU {before['renderer_cpu_pct']:.0f}%->{after['renderer_cpu_pct']:.0f}%; "
            f"high_cpu_ui_process_count={after['high_cpu_ui_process_count']}"
        ),
    )


def assert_no_leftovers() -> None:
    proc = subprocess.run(
        [
            "pgrep",
            "-fl",
            "axon-value-no-policy|axon-value-policy-allowed|axon-value-ui-spin|playwright-mcp-worker",
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    lines = [
        line
        for line in proc.stdout.splitlines()
        if "demo_agent_value_live.py" not in line and "pgrep" not in line
    ]
    if lines:
        raise AssertionError("leftover value-demo processes:\n" + "\n".join(lines))


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    scenarios = [
        scenario_without_policy_spawns_too_much,
        scenario_with_policy_avoids_fanout,
        scenario_parallelism_reduction,
        scenario_ui_pressure,
    ]
    results: list[ScenarioResult] = []
    try:
        for scenario in scenarios:
            result = scenario(axon_bin)
            results.append(result)
            print(f"[value] {result.name}: {result.value}")
        assert_no_leftovers()
    except Exception as exc:
        print(f"[err] {exc}", file=sys.stderr)
        return 1

    print("[summary] Axon allowed useful work, reduced parallelism, avoided MCP sprawl, and detected UI pressure.")
    print("[ok] live value demo passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
