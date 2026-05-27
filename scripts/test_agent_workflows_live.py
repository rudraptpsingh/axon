#!/usr/bin/env python3
"""Exhaustive live workflow proofs for Axon agent utility.

These scenarios intentionally create local-only agent failure modes, ask Axon,
and assert that an agent policy would change behavior or surface the right
workflow impact.
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


def has_impact(data: dict, use_case: str) -> bool:
    return any(impact.get("use_case") == use_case for impact in data.get("workflow_impacts", []))


def spawn_playwright_mcp(count: int) -> tuple[tempfile.TemporaryDirectory[str], list[subprocess.Popen[str]]]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required")

    tmp = tempfile.TemporaryDirectory(prefix="axon-workflow-mcp-")
    script = Path(tmp.name) / "playwright-mcp-worker.js"
    script.write_text("setInterval(() => {}, 1000);\n", encoding="utf-8")
    children = [
        subprocess.Popen(
            [node, str(script), "--axon-workflow-duplicate-mcp-proof"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        for _ in range(count)
    ]
    return tmp, children


def spawn_codex_renderer_spin(seconds: int = 20) -> subprocess.Popen[str]:
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
            "--axon-ui-spin-proof",
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


def scenario_duplicate_mcp_blocks_agent_spawn(axon_bin: str) -> None:
    before = run_axon(axon_bin, "agent_runtime_health")["data"]
    before_playwright = group_count(before["duplicate_mcp_server_groups"], "playwright-mcp")
    before_mcp = before["mcp_server_count"]

    tmp, children = spawn_playwright_mcp(2)
    try:
        time.sleep(1)
        after = run_axon(axon_bin, "agent_runtime_health")["data"]
        after_playwright = group_count(after["duplicate_mcp_server_groups"], "playwright-mcp")
        after_mcp = after["mcp_server_count"]

        should_spawn = not after["duplicate_mcp_server_groups"] and after["stale_mcp_server_count"] < 8
        print(
            f"[scenario duplicate-mcp] playwright={before_playwright}->{after_playwright} "
            f"mcp={before_mcp}->{after_mcp} agent_decision={'spawn' if should_spawn else 'do_not_spawn'}"
        )

        if after_playwright < before_playwright + 2:
            raise AssertionError("Axon did not detect the injected duplicate playwright-mcp helpers")
        if after_mcp < before_mcp + 2:
            raise AssertionError("Axon did not detect the injected MCP helper count")
        if should_spawn:
            raise AssertionError("agent policy should block new MCP spawns under duplicate MCP pressure")
        if not has_impact(after, "Long agent coding session"):
            raise AssertionError("missing Long agent coding session workflow impact")
    finally:
        cleanup(children, tmp)


def scenario_workload_advice_gates_heavy_job(axon_bin: str) -> None:
    workload = run_axon(axon_bin, "workload_advice")["data"]
    recommendation = workload["recommendation"]
    would_start_heavy_job = recommendation not in {"defer", "cooldown"}
    print(
        f"[scenario workload-gate] recommendation={recommendation} "
        f"agent_decision={'start_heavy_job' if would_start_heavy_job else 'do_not_start_heavy_job'}"
    )

    if recommendation in {"defer", "cooldown"} and would_start_heavy_job:
        raise AssertionError("agent ignored Axon's defer/cooldown recommendation")


def scenario_ui_cpu_pressure_surfaces_ide_impact(axon_bin: str) -> None:
    before = run_axon(axon_bin, "agent_runtime_health")["data"]
    before_renderer = before["renderer_cpu_pct"]

    child = spawn_codex_renderer_spin()
    try:
        time.sleep(1)
        after = run_axon(axon_bin, "agent_runtime_health")["data"]
        print(
            f"[scenario ui-spin] renderer_cpu={before_renderer:.0f}->{after['renderer_cpu_pct']:.0f} "
            f"high_cpu_ui={after['high_cpu_ui_process_count']}"
        )
        if after["renderer_cpu_pct"] < before_renderer + 20.0 and after["high_cpu_ui_process_count"] < 1:
            raise AssertionError("Axon did not detect injected renderer CPU pressure")
        if not has_impact(after, "Interactive IDE or desktop-agent work"):
            raise AssertionError("missing Interactive IDE or desktop-agent work impact")
    finally:
        cleanup([child])


def assert_no_leftovers() -> None:
    proc = subprocess.run(
        [
            "pgrep",
            "-fl",
            "axon-workflow-duplicate-mcp-proof|axon-ui-spin-proof|playwright-mcp-worker",
        ],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
    )
    lines = [
        line
        for line in proc.stdout.splitlines()
        if "test_agent_workflows_live.py" not in line and "pgrep" not in line
    ]
    if lines:
        raise AssertionError("leftover proof processes:\n" + "\n".join(lines))


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    scenarios = [
        scenario_duplicate_mcp_blocks_agent_spawn,
        scenario_workload_advice_gates_heavy_job,
        scenario_ui_cpu_pressure_surfaces_ide_impact,
    ]
    try:
        for scenario in scenarios:
            scenario(axon_bin)
        assert_no_leftovers()
    except Exception as exc:
        print(f"[err] {exc}", file=sys.stderr)
        return 1

    print("[ok] all live agent workflow scenarios passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
