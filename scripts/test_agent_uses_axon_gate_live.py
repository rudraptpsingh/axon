#!/usr/bin/env python3
"""Live proof: an agent changes behavior based on Axon's output.

This script is a tiny stand-in for Codex/Claude/Cursor policy code:
before spawning another MCP-heavy tool stack, it calls Axon's
agent_runtime_health and workload_advice outputs. If Axon reports duplicate
or stale MCP pressure, the agent refuses to spawn more helpers and emits the
user-facing action it would take instead.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
from pathlib import Path


def run_axon(axon_bin: str, tool: str) -> dict:
    proc = subprocess.run(
        [axon_bin, "query", tool],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=30,
        check=True,
    )
    return json.loads(proc.stdout)


def should_spawn_more_mcp(runtime: dict, workload: dict) -> tuple[bool, str]:
    data = runtime["data"]
    advice = workload["data"]
    impacts = data.get("workflow_impacts", [])

    if advice.get("recommendation") in {"defer", "cooldown"}:
        return (
            False,
            "Axon workload_advice says to defer heavy work before spawning more MCP tools.",
        )

    if data.get("stale_mcp_server_count", 0) >= 8:
        return (
            False,
            "Axon found stale MCP servers; restart the agent host before spawning more tools.",
        )

    if data.get("duplicate_mcp_server_groups"):
        return (
            False,
            "Axon found duplicated MCP stacks; reuse or clean existing tools instead of spawning more.",
        )

    for impact in impacts:
        if impact.get("use_case") == "Long agent coding session":
            return False, impact.get("recommended_action", "Clean up agent runtime first.")

    return True, "Axon says the agent runtime is ready for another MCP-heavy task."


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    runtime = run_axon(axon_bin, "agent_runtime_health")
    workload = run_axon(axon_bin, "workload_advice")
    allow_spawn, reason = should_spawn_more_mcp(runtime, workload)

    before_count = runtime["data"]["mcp_server_count"]
    print(f"[agent] asked Axon before spawning another MCP-heavy tool stack")
    print(f"[agent] Axon MCP servers={before_count}, stale={runtime['data']['stale_mcp_server_count']}")
    print(f"[agent] Axon workload recommendation={workload['data']['recommendation']}")
    print(f"[agent] decision={'spawn' if allow_spawn else 'do_not_spawn'}")
    print(f"[agent] reason={reason}")

    if allow_spawn:
        node = shutil.which("node")
        if not node:
            print("[err] node missing, cannot prove spawn path", file=sys.stderr)
            return 2
        child = subprocess.Popen(
            [node, "-e", "setInterval(() => {}, 1000)", "--axon-agent-gate-proof"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        try:
            after_count = run_axon(axon_bin, "agent_runtime_health")["data"]["mcp_server_count"]
        finally:
            child.terminate()
            try:
                child.wait(timeout=3)
            except subprocess.TimeoutExpired:
                child.kill()
        if after_count <= before_count:
            print("[err] expected spawn path to increase runtime count", file=sys.stderr)
            return 1
        print("[ok] agent spawned after Axon allowed it")
        return 0

    after_count = run_axon(axon_bin, "agent_runtime_health")["data"]["mcp_server_count"]
    if after_count > before_count:
        print("[err] MCP count increased even though agent gate denied spawning", file=sys.stderr)
        return 1

    print("[ok] agent changed behavior based on Axon and did not spawn extra MCP tools")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
