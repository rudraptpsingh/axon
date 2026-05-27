#!/usr/bin/env python3
"""Live proof: Axon detects duplicated MCP-style helper processes.

This is intentionally small and local-only. It starts two temporary Node
processes whose command lines look like a Playwright MCP server, runs
`axon query agent_runtime_health`, verifies the duplicate MCP count increases,
then terminates the helpers.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
import time
from pathlib import Path


def run_axon(axon_bin: str) -> dict:
    proc = subprocess.run(
        [axon_bin, "query", "agent_runtime_health"],
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=30,
        check=True,
    )
    return json.loads(proc.stdout)


def group_count(groups: list[str], name: str) -> int:
    prefix = f"{name} x"
    for group in groups:
        if group.startswith(prefix):
            return int(group[len(prefix) :])
    return 0


def spawn_fake_mcp_helpers(count: int) -> tuple[tempfile.TemporaryDirectory[str], list[subprocess.Popen[str]]]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required for this live test")

    tmp = tempfile.TemporaryDirectory(prefix="axon-mcp-proof-")
    script = Path(tmp.name) / "playwright-mcp-worker.js"
    script.write_text(
        "setInterval(() => {}, 1000);\n",
        encoding="utf-8",
    )

    children: list[subprocess.Popen[str]] = []
    for _ in range(count):
        children.append(
            subprocess.Popen(
                [node, str(script), "--axon-live-duplicate-mcp-proof"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
                text=True,
            )
        )
    return tmp, children


def cleanup(children: list[subprocess.Popen[str]], tmp: tempfile.TemporaryDirectory[str]) -> None:
    for child in children:
        if child.poll() is None:
            child.terminate()
    deadline = time.time() + 3
    for child in children:
        remaining = max(0.1, deadline - time.time())
        try:
            child.wait(timeout=remaining)
        except subprocess.TimeoutExpired:
            child.kill()
    tmp.cleanup()


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2

    before = run_axon(axon_bin)["data"]
    before_playwright = group_count(before["duplicate_mcp_server_groups"], "playwright-mcp")
    before_mcp = before["mcp_server_count"]

    tmp, children = spawn_fake_mcp_helpers(2)
    try:
        time.sleep(1)
        after = run_axon(axon_bin)["data"]
        after_playwright = group_count(after["duplicate_mcp_server_groups"], "playwright-mcp")
        after_mcp = after["mcp_server_count"]
        impacts = after.get("workflow_impacts", [])

        print(f"[info] playwright-mcp duplicate count: {before_playwright} -> {after_playwright}")
        print(f"[info] total MCP server count: {before_mcp} -> {after_mcp}")
        for impact in impacts:
            print(
                "[impact] {use_case}: {business_impact} action={recommended_action}".format(
                    **impact
                )
            )

        if after_playwright < before_playwright + 2:
            print("[err] expected playwright-mcp duplicate count to increase by at least 2", file=sys.stderr)
            return 1
        if after_mcp < before_mcp + 2:
            print("[err] expected total MCP server count to increase by at least 2", file=sys.stderr)
            return 1
        if not any(impact.get("use_case") == "Long agent coding session" for impact in impacts):
            print("[err] expected a Long agent coding session workflow impact", file=sys.stderr)
            return 1

        print("[ok] live duplicate MCP scenario detected and translated into workflow impact")
        return 0
    finally:
        cleanup(children, tmp)


if __name__ == "__main__":
    raise SystemExit(main())
