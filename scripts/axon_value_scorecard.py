#!/usr/bin/env python3
"""Create a metric-driven Axon value scorecard.

This report is meant for users and buyers, not kernel nerds. It translates
Axon's local runtime signals into avoided work, time, and estimated credit/cost
impact for agentic coding/app-building workflows.
"""

from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass
from pathlib import Path


@dataclass
class ValueInputs:
    requested_tool_spawns: int
    requested_parallelism: int
    credit_per_tool_spawn: float
    credit_per_parallel_worker: float
    seconds_per_tool_spawn: float
    seconds_per_parallel_worker: float
    hourly_developer_cost: float


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
        raise RuntimeError("node is required for --prove")
    tmp = tempfile.TemporaryDirectory(prefix="axon-scorecard-tool-")
    script = Path(tmp.name) / "playwright-mcp-worker.js"
    script.write_text("setInterval(() => {}, 1000);\n", encoding="utf-8")
    children = [
        subprocess.Popen(
            [node, str(script), "--axon-scorecard-tool-worker"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        for _ in range(count)
    ]
    return tmp, children


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


def measured_no_policy_fanout(axon_bin: str, requested_tool_spawns: int) -> dict:
    before = run_axon(axon_bin, "agent_runtime_health")["data"]
    before_mcp = before["mcp_server_count"]
    before_playwright = group_count(before["duplicate_mcp_server_groups"], "playwright-mcp")
    tmp, children = spawn_tool_workers(requested_tool_spawns)
    try:
        time.sleep(1)
        during = run_axon(axon_bin, "agent_runtime_health")["data"]
    finally:
        cleanup(children, tmp)
    time.sleep(0.5)
    after = run_axon(axon_bin, "agent_runtime_health")["data"]
    return {
        "before_mcp": before_mcp,
        "during_mcp": during["mcp_server_count"],
        "after_mcp": after["mcp_server_count"],
        "before_playwright": before_playwright,
        "during_playwright": group_count(during["duplicate_mcp_server_groups"], "playwright-mcp"),
    }


def compute_scorecard(runtime: dict, workload: dict, inputs: ValueInputs, proof: dict | None) -> dict:
    data = runtime["data"]
    advice = workload["data"]
    duplicate_groups = data.get("duplicate_mcp_server_groups", [])
    stale_mcp = int(data.get("stale_mcp_server_count", 0))
    safe_parallelism = int(advice.get("safe_parallelism") or inputs.requested_parallelism)
    safe_parallelism = max(1, min(inputs.requested_parallelism, safe_parallelism))

    block_new_tools = bool(duplicate_groups) or stale_mcp >= 8 or advice.get("recommendation") in {
        "defer",
        "cooldown",
    }
    risky_tool_spawns_avoided = inputs.requested_tool_spawns if block_new_tools else 0
    parallel_workers_avoided = max(0, inputs.requested_parallelism - safe_parallelism)

    estimated_credits_saved = (
        risky_tool_spawns_avoided * inputs.credit_per_tool_spawn
        + parallel_workers_avoided * inputs.credit_per_parallel_worker
    )
    estimated_minutes_saved = (
        risky_tool_spawns_avoided * inputs.seconds_per_tool_spawn
        + parallel_workers_avoided * inputs.seconds_per_parallel_worker
    ) / 60.0
    estimated_developer_cost_saved = estimated_minutes_saved / 60.0 * inputs.hourly_developer_cost

    risk_reasons = []
    if duplicate_groups:
        risk_reasons.append("duplicate MCP/tool groups: " + ", ".join(duplicate_groups))
    if stale_mcp >= 8:
        risk_reasons.append(f"{stale_mcp} stale MCP/tool servers")
    if advice.get("recommendation") in {"defer", "cooldown"}:
        risk_reasons.append(f"workload advice={advice.get('recommendation')}")
    if data.get("high_cpu_ui_process_count", 0) > 0:
        risk_reasons.append("agent UI/runtime CPU pressure")

    return {
        "current_state": {
            "agent_processes": data.get("process_count"),
            "mcp_servers": data.get("mcp_server_count"),
            "stale_mcp_servers": stale_mcp,
            "mcp_ram_mb": data.get("mcp_total_ram_mb"),
            "total_agent_cpu_pct": data.get("total_cpu_pct"),
            "total_agent_ram_mb": data.get("total_ram_mb"),
            "workload_recommendation": advice.get("recommendation"),
            "safe_parallelism": safe_parallelism,
        },
        "value_metrics": {
            "risky_tool_spawns_avoided": risky_tool_spawns_avoided,
            "parallel_workers_avoided": parallel_workers_avoided,
            "estimated_credits_saved": round(estimated_credits_saved, 2),
            "estimated_minutes_saved": round(estimated_minutes_saved, 2),
            "estimated_developer_cost_saved_usd": round(estimated_developer_cost_saved, 2),
        },
        "risk_reasons": risk_reasons,
        "workflow_impacts": data.get("workflow_impacts", []),
        "proof": proof,
    }


def render_markdown(scorecard: dict) -> str:
    state = scorecard["current_state"]
    value = scorecard["value_metrics"]
    lines = [
        "# Axon Value Scorecard",
        "",
        "## Current Agent Runtime",
        "",
        f"- Agent processes: `{state['agent_processes']}`",
        f"- MCP/tool servers: `{state['mcp_servers']}`",
        f"- Stale MCP/tool servers: `{state['stale_mcp_servers']}`",
        f"- MCP/tool RAM: `{state['mcp_ram_mb']:.0f} MB`",
        f"- Agent CPU: `{state['total_agent_cpu_pct']:.0f}%`",
        f"- Agent RAM: `{state['total_agent_ram_mb']:.0f} MB`",
        f"- Workload recommendation: `{state['workload_recommendation']}`",
        f"- Safe parallelism: `{state['safe_parallelism']}`",
        "",
        "## Value Created",
        "",
        f"- Risky tool spawns avoided: `{value['risky_tool_spawns_avoided']}`",
        f"- Parallel workers avoided: `{value['parallel_workers_avoided']}`",
        f"- Estimated credits saved: `{value['estimated_credits_saved']}`",
        f"- Estimated time saved: `{value['estimated_minutes_saved']} min`",
        f"- Estimated developer cost avoided: `${value['estimated_developer_cost_saved_usd']}`",
        "",
        "## Why Axon Acted",
        "",
    ]
    if scorecard["risk_reasons"]:
        lines.extend(f"- {reason}" for reason in scorecard["risk_reasons"])
    else:
        lines.append("- No current action threshold was crossed.")

    if scorecard.get("proof"):
        proof = scorecard["proof"]
        lines.extend(
            [
                "",
                "## Live Proof",
                "",
                f"- No-policy MCP count: `{proof['before_mcp']} -> {proof['during_mcp']}`",
                f"- Cleanup MCP count: `{proof['after_mcp']}`",
                f"- Playwright/tool group: `{proof['before_playwright']} -> {proof['during_playwright']}`",
            ]
        )

    impacts = scorecard.get("workflow_impacts", [])
    if impacts:
        lines.extend(["", "## Workflow Impact", ""])
        for impact in impacts[:4]:
            lines.append(f"- **{impact['use_case']}**: {impact['business_impact']}")
            lines.append(f"  Action: {impact['recommended_action']}")

    lines.extend(
        [
            "",
            "## User-Facing Summary",
            "",
            (
                f"Axon avoided {value['risky_tool_spawns_avoided']} risky tool spawns, "
                f"reduced parallelism by {value['parallel_workers_avoided']} workers, and "
                f"estimated {value['estimated_minutes_saved']} minutes of wasted work avoided."
            ),
        ]
    )
    return "\n".join(lines) + "\n"


def parse_args() -> argparse.Namespace:
    parser = argparse.ArgumentParser(description="Generate a metric-driven Axon value scorecard.")
    parser.add_argument("axon_bin", nargs="?", default="target/debug/axon")
    parser.add_argument("--requested-tool-spawns", type=int, default=4)
    parser.add_argument("--requested-parallelism", type=int, default=4)
    parser.add_argument("--credit-per-tool-spawn", type=float, default=1.0)
    parser.add_argument("--credit-per-parallel-worker", type=float, default=0.25)
    parser.add_argument("--seconds-per-tool-spawn", type=float, default=45.0)
    parser.add_argument("--seconds-per-parallel-worker", type=float, default=30.0)
    parser.add_argument("--hourly-developer-cost", type=float, default=100.0)
    parser.add_argument("--prove", action="store_true", help="Inject temporary tool workers to prove no-policy fan-out.")
    parser.add_argument("--json", action="store_true", help="Print JSON instead of Markdown.")
    parser.add_argument("--out", help="Optional output path.")
    return parser.parse_args()


def main() -> int:
    args = parse_args()
    if not Path(args.axon_bin).exists() and shutil.which(args.axon_bin) is None:
        print(f"[err] axon binary not found: {args.axon_bin}", file=sys.stderr)
        return 2

    inputs = ValueInputs(
        requested_tool_spawns=args.requested_tool_spawns,
        requested_parallelism=args.requested_parallelism,
        credit_per_tool_spawn=args.credit_per_tool_spawn,
        credit_per_parallel_worker=args.credit_per_parallel_worker,
        seconds_per_tool_spawn=args.seconds_per_tool_spawn,
        seconds_per_parallel_worker=args.seconds_per_parallel_worker,
        hourly_developer_cost=args.hourly_developer_cost,
    )

    proof = measured_no_policy_fanout(args.axon_bin, args.requested_tool_spawns) if args.prove else None
    runtime = run_axon(args.axon_bin, "agent_runtime_health")
    workload = run_axon(args.axon_bin, "workload_advice")
    scorecard = compute_scorecard(runtime, workload, inputs, proof)
    output = json.dumps(scorecard, indent=2) + "\n" if args.json else render_markdown(scorecard)

    if args.out:
        Path(args.out).write_text(output, encoding="utf-8")
        print(f"[ok] wrote Axon value scorecard to {args.out}")
    else:
        print(output, end="")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
