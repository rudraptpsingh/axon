#!/usr/bin/env python3
"""A/B app-development scenario: blind agent vs Axon-informed agent.

Each run uses an independent temp workspace and builds/tests the same small web
app. The blind run follows a naive policy: spawn all requested tool helpers and
run all checks in parallel. The Axon run asks Axon first, avoids risky tool
fan-out when runtime pressure is high, and caps parallelism to Axon's safe
parallelism. Both runs are instrumented and compared at the end.
"""

from __future__ import annotations

import json
import hashlib
import os
import shutil
import subprocess
import sys
import tempfile
import time
from dataclasses import dataclass, field
from pathlib import Path
from typing import Callable


REQUESTED_TOOL_HELPERS = 4
REQUESTED_PARALLELISM = 4
ESTIMATED_CREDIT_PER_TOOL_HELPER = 1.0
ESTIMATED_CREDIT_PER_EXTRA_PARALLEL_WORKER = 0.25
ESTIMATED_RISK_MINUTES_PER_TOOL_HELPER = 1.0


@dataclass
class RunMetrics:
    mode: str
    workspace: str
    elapsed_s: float
    app_tests_passed: bool
    requested_tool_helpers: int
    actual_tool_helpers: int
    avoided_tool_helpers: int
    requested_parallelism: int
    actual_parallelism: int
    max_check_concurrency: int
    subprocesses_started: int
    app_output_hash: str
    app_benchmark_ms: float
    axon_recommendation: str | None = None
    axon_safe_parallelism: int | None = None
    mcp_before: int | None = None
    mcp_after: int | None = None
    stale_mcp_before: int | None = None
    workflow_impacts: list[str] = field(default_factory=list)


@dataclass
class FastPathMetrics:
    blind_elapsed_s: float
    axon_elapsed_s: float
    blind_validation_s: float
    axon_validation_s: float
    blind_tool_helpers: int
    axon_tool_helpers: int
    tests_passed: bool


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


def write_app(workspace: Path) -> None:
    (workspace / "src").mkdir()
    (workspace / "test").mkdir()
    (workspace / "public").mkdir()
    (workspace / "package.json").write_text(
        json.dumps(
            {
                "name": "axon-demo-app",
                "version": "1.0.0",
                "type": "module",
                "scripts": {
                    "test": "node --test test/app.test.js",
                    "smoke": "node test/smoke.test.js",
                },
            },
            indent=2,
        )
        + "\n",
        encoding="utf-8",
    )
    (workspace / "src" / "app.js").write_text(
        """
export function createTask(title, priority = "normal") {
  if (!title || !title.trim()) throw new Error("title required");
  return { id: title.toLowerCase().replaceAll(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, ""), title, priority, done: false };
}

export function summarize(tasks) {
  const total = tasks.length;
  const done = tasks.filter((task) => task.done).length;
  const urgent = tasks.filter((task) => task.priority === "urgent" && !task.done).length;
  return { total, done, urgent, completionPct: total === 0 ? 0 : Math.round((done / total) * 100) };
}

export function renderTaskList(tasks) {
  return `<ul>${tasks.map((task) => `<li data-priority="${task.priority}">${task.title}</li>`).join("")}</ul>`;
}
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace / "public" / "index.html").write_text(
        """
<!doctype html>
<html>
  <head><meta charset="utf-8"><title>Axon Demo App</title></head>
  <body>
    <main>
      <h1>Launch Checklist</h1>
      <div id="app"></div>
      <script type="module">
        import { createTask, renderTaskList } from "../src/app.js";
        const tasks = [createTask("Ship app", "urgent"), createTask("Run smoke tests")];
        document.querySelector("#app").innerHTML = renderTaskList(tasks);
      </script>
    </main>
  </body>
</html>
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace / "test" / "app.test.js").write_text(
        """
import test from "node:test";
import assert from "node:assert/strict";
import { createTask, summarize, renderTaskList } from "../src/app.js";

test("creates stable task ids", () => {
  assert.equal(createTask("Ship MVP!").id, "ship-mvp");
});

test("summarizes completion and urgent work", () => {
  const tasks = [createTask("A", "urgent"), { ...createTask("B"), done: true }];
  assert.deepEqual(summarize(tasks), { total: 2, done: 1, urgent: 1, completionPct: 50 });
});

test("renders task list", () => {
  assert.match(renderTaskList([createTask("Deploy", "urgent")]), /data-priority="urgent"/);
});
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace / "test" / "smoke.test.js").write_text(
        """
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";

const html = readFileSync("public/index.html", "utf8");
assert.match(html, /Launch Checklist/);
assert.match(html, /type="module"/);
console.log("smoke ok");
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace / "test" / "lint.mjs").write_text(
        """
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
const app = readFileSync("src/app.js", "utf8");
assert(!app.includes("TODO_FAIL"));
assert(app.includes("createTask"));
console.log("lint ok");
""".strip()
        + "\n",
        encoding="utf-8",
    )
    (workspace / "test" / "bundle-check.mjs").write_text(
        """
import assert from "node:assert/strict";
const mod = await import("../src/app.js");
assert.equal(typeof mod.createTask, "function");
assert.equal(typeof mod.summarize, "function");
console.log("bundle check ok");
""".strip()
        + "\n",
        encoding="utf-8",
    )


def spawn_tool_helpers(workspace: Path, count: int) -> list[subprocess.Popen[str]]:
    node = shutil.which("node")
    if not node:
        raise RuntimeError("node is required")
    script = workspace / "playwright-mcp-worker.js"
    script.write_text("setInterval(() => {}, 1000);\n", encoding="utf-8")
    return [
        subprocess.Popen(
            [node, str(script), f"--axon-appdev-{workspace.name}"],
            cwd=workspace,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
            text=True,
        )
        for _ in range(count)
    ]


def terminate_all(children: list[subprocess.Popen[str]]) -> None:
    for child in children:
        if child.poll() is None:
            child.terminate()
    deadline = time.time() + 4
    for child in children:
        try:
            child.wait(timeout=max(0.1, deadline - time.time()))
        except subprocess.TimeoutExpired:
            child.kill()


def run_checks(workspace: Path, parallelism: int) -> tuple[bool, int, int]:
    commands = [
        ["node", "--test", "test/app.test.js"],
        ["node", "test/smoke.test.js"],
        ["node", "test/lint.mjs"],
        ["node", "test/bundle-check.mjs"],
    ]
    pending = list(commands)
    running: list[subprocess.Popen[str]] = []
    max_concurrency = 0
    started = 0
    passed = True

    while pending or running:
        while pending and len(running) < parallelism:
            cmd = pending.pop(0)
            running.append(
                subprocess.Popen(
                    cmd,
                    cwd=workspace,
                    stdout=subprocess.PIPE,
                    stderr=subprocess.PIPE,
                    text=True,
                )
            )
            started += 1
            max_concurrency = max(max_concurrency, len(running))

        still_running = []
        for proc in running:
            if proc.poll() is None:
                still_running.append(proc)
                continue
            stdout, stderr = proc.communicate()
            if proc.returncode != 0:
                passed = False
                print(f"[check failed] {' '.join(proc.args)}", file=sys.stderr)
                print(stdout, file=sys.stderr)
                print(stderr, file=sys.stderr)
        running = still_running
        time.sleep(0.03)

    return passed, max_concurrency, started


def app_output_hash(workspace: Path) -> str:
    digest = hashlib.sha256()
    for rel in ["src/app.js", "public/index.html", "test/app.test.js"]:
        digest.update(rel.encode("utf-8"))
        digest.update((workspace / rel).read_bytes())
    return digest.hexdigest()[:16]


def benchmark_app(workspace: Path) -> float:
    script = """
const { createTask, summarize, renderTaskList } = await import('./src/app.js');
const tasks = Array.from({ length: 200 }, (_, i) => createTask(`Task ${i}`, i % 7 === 0 ? 'urgent' : 'normal'));
const start = performance.now();
for (let i = 0; i < 5000; i++) {
  summarize(tasks);
  renderTaskList(tasks.slice(0, 20));
}
const elapsed = performance.now() - start;
console.log(JSON.stringify({ elapsed }));
"""
    proc = subprocess.run(
        ["node", "-e", script],
        cwd=workspace,
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        timeout=20,
        check=True,
    )
    return float(json.loads(proc.stdout)["elapsed"])


def run_agent(mode: str, axon_bin: str | None) -> RunMetrics:
    start = time.perf_counter()
    tmp = tempfile.TemporaryDirectory(prefix=f"axon-appdev-{mode}-")
    workspace = Path(tmp.name)
    tool_helpers: list[subprocess.Popen[str]] = []

    runtime_before = None
    workload = None
    recommendation = None
    safe_parallelism = REQUESTED_PARALLELISM
    actual_tool_helpers = REQUESTED_TOOL_HELPERS
    actual_parallelism = REQUESTED_PARALLELISM
    impacts: list[str] = []

    try:
        if mode == "axon":
            assert axon_bin is not None
            runtime_before = run_axon(axon_bin, "agent_runtime_health")["data"]
            workload = run_axon(axon_bin, "workload_advice")["data"]
            recommendation = workload["recommendation"]
            safe_parallelism = int(workload.get("safe_parallelism") or REQUESTED_PARALLELISM)
            safe_parallelism = max(1, min(REQUESTED_PARALLELISM, safe_parallelism))
            duplicate_groups = runtime_before.get("duplicate_mcp_server_groups", [])
            stale_mcp = runtime_before.get("stale_mcp_server_count", 0)
            if duplicate_groups or stale_mcp >= 8 or recommendation in {"defer", "cooldown"}:
                actual_tool_helpers = 0
            actual_parallelism = safe_parallelism
            impacts = [impact["use_case"] for impact in runtime_before.get("workflow_impacts", [])]

        write_app(workspace)
        tool_helpers = spawn_tool_helpers(workspace, actual_tool_helpers)
        time.sleep(0.4 if actual_tool_helpers else 0.0)
        passed, max_concurrency, checks_started = run_checks(workspace, actual_parallelism)
        output_hash = app_output_hash(workspace)
        benchmark_ms = benchmark_app(workspace)
        terminate_all(tool_helpers)
        tool_helpers = []

        runtime_after = run_axon(axon_bin, "agent_runtime_health")["data"] if axon_bin else None
        elapsed = time.perf_counter() - start
        return RunMetrics(
            mode=mode,
            workspace=str(workspace),
            elapsed_s=elapsed,
            app_tests_passed=passed,
            requested_tool_helpers=REQUESTED_TOOL_HELPERS,
            actual_tool_helpers=actual_tool_helpers,
            avoided_tool_helpers=REQUESTED_TOOL_HELPERS - actual_tool_helpers,
            requested_parallelism=REQUESTED_PARALLELISM,
            actual_parallelism=actual_parallelism,
            max_check_concurrency=max_concurrency,
            subprocesses_started=checks_started + actual_tool_helpers,
            app_output_hash=output_hash,
            app_benchmark_ms=benchmark_ms,
            axon_recommendation=recommendation,
            axon_safe_parallelism=safe_parallelism if mode == "axon" else None,
            mcp_before=runtime_before.get("mcp_server_count") if runtime_before else None,
            mcp_after=runtime_after.get("mcp_server_count") if runtime_after else None,
            stale_mcp_before=runtime_before.get("stale_mcp_server_count") if runtime_before else None,
            workflow_impacts=impacts,
        )
    finally:
        terminate_all(tool_helpers)
        tmp.cleanup()


def run_fast_path_once(mode: str, axon_bin: str) -> tuple[float, float, int, bool]:
    start = time.perf_counter()
    tmp = tempfile.TemporaryDirectory(prefix=f"axon-fastpath-{mode}-")
    workspace = Path(tmp.name)
    helpers: list[subprocess.Popen[str]] = []
    helper_count = 0
    try:
        write_app(workspace)
        if mode == "blind":
            helper_count = REQUESTED_TOOL_HELPERS
            helpers = spawn_tool_helpers(workspace, helper_count)
            time.sleep(0.4)
        else:
            runtime = run_axon(axon_bin, "agent_runtime_health")["data"]
            # Lightweight smoke checks do not need new browser/MCP helpers. Axon
            # policy skips them even on a pressured host, while still running
            # useful app validation.
            helper_count = 0 if runtime.get("duplicate_mcp_server_groups") else 1
            if helper_count:
                helpers = spawn_tool_helpers(workspace, helper_count)
                time.sleep(0.1)
        validation_start = time.perf_counter()
        passed, _, _ = run_checks(workspace, 1)
        validation_elapsed = time.perf_counter() - validation_start
        elapsed = time.perf_counter() - start
        return elapsed, validation_elapsed, helper_count, passed
    finally:
        terminate_all(helpers)
        tmp.cleanup()


def run_fast_path(axon_bin: str) -> FastPathMetrics:
    blind_elapsed, blind_validation, blind_helpers, blind_passed = run_fast_path_once(
        "blind", axon_bin
    )
    axon_elapsed, axon_validation, axon_helpers, axon_passed = run_fast_path_once(
        "axon", axon_bin
    )
    return FastPathMetrics(
        blind_elapsed_s=blind_elapsed,
        axon_elapsed_s=axon_elapsed,
        blind_validation_s=blind_validation,
        axon_validation_s=axon_validation,
        blind_tool_helpers=blind_helpers,
        axon_tool_helpers=axon_helpers,
        tests_passed=blind_passed and axon_passed,
    )


def metrics_to_dict(metrics: RunMetrics) -> dict:
    return {
        "mode": metrics.mode,
        "elapsed_s": round(metrics.elapsed_s, 3),
        "app_tests_passed": metrics.app_tests_passed,
        "requested_tool_helpers": metrics.requested_tool_helpers,
        "actual_tool_helpers": metrics.actual_tool_helpers,
        "avoided_tool_helpers": metrics.avoided_tool_helpers,
        "requested_parallelism": metrics.requested_parallelism,
        "actual_parallelism": metrics.actual_parallelism,
        "max_check_concurrency": metrics.max_check_concurrency,
        "subprocesses_started": metrics.subprocesses_started,
        "app_output_hash": metrics.app_output_hash,
        "app_benchmark_ms": round(metrics.app_benchmark_ms, 3),
        "axon_recommendation": metrics.axon_recommendation,
        "axon_safe_parallelism": metrics.axon_safe_parallelism,
        "mcp_before": metrics.mcp_before,
        "mcp_after": metrics.mcp_after,
        "stale_mcp_before": metrics.stale_mcp_before,
        "workflow_impacts": metrics.workflow_impacts,
    }


def compare(blind: RunMetrics, axon: RunMetrics) -> dict:
    tool_helpers_avoided = blind.actual_tool_helpers - axon.actual_tool_helpers
    subprocesses_avoided = blind.subprocesses_started - axon.subprocesses_started
    parallelism_reduction = blind.max_check_concurrency - axon.max_check_concurrency
    estimated_credits_saved = (
        tool_helpers_avoided * ESTIMATED_CREDIT_PER_TOOL_HELPER
        + max(0, parallelism_reduction) * ESTIMATED_CREDIT_PER_EXTRA_PARALLEL_WORKER
    )
    estimated_risk_minutes_avoided = tool_helpers_avoided * ESTIMATED_RISK_MINUTES_PER_TOOL_HELPER
    return {
        "both_completed": blind.app_tests_passed and axon.app_tests_passed,
        "app_output_identical": blind.app_output_hash == axon.app_output_hash,
        "app_benchmark_delta_ms": round(axon.app_benchmark_ms - blind.app_benchmark_ms, 3),
        "tool_helpers_avoided": tool_helpers_avoided,
        "subprocesses_avoided": subprocesses_avoided,
        "parallelism_reduction": parallelism_reduction,
        "elapsed_delta_s": round(axon.elapsed_s - blind.elapsed_s, 3),
        "estimated_credits_saved": round(estimated_credits_saved, 2),
        "estimated_risk_minutes_avoided": round(estimated_risk_minutes_avoided, 2),
        "tradeoff": (
            "Axon intentionally ran slower on this pressured machine to avoid extra tool fan-out and reduce concurrency."
            if axon.elapsed_s > blind.elapsed_s
            else "Axon reduced risk without increasing elapsed time in this run."
        ),
        "interpretation": (
            "Axon preserved app-development success while reducing risky helper fan-out and parallelism."
            if blind.app_tests_passed and axon.app_tests_passed and blind.app_output_hash == axon.app_output_hash
            else "One scenario failed; inspect run metrics."
        ),
    }


def render_report(
    blind: RunMetrics, axon: RunMetrics, comparison: dict, fast_path: FastPathMetrics
) -> str:
    fast_delta = fast_path.axon_elapsed_s - fast_path.blind_elapsed_s
    validation_delta = fast_path.axon_validation_s - fast_path.blind_validation_s
    lines = [
        "# App Development A/B: Without Axon vs With Axon",
        "",
        "## Scenario",
        "",
        "Both agents build and test the same small web app in independent temp workspaces.",
        "The blind agent spawns all requested tool helpers and runs all checks in parallel.",
        "The Axon-informed agent asks Axon first, avoids risky tool fan-out, and caps parallelism.",
        "",
        "## Results",
        "",
        "| Metric | Blind agent | Axon-informed agent |",
        "| --- | ---: | ---: |",
        f"| App tests passed | {blind.app_tests_passed} | {axon.app_tests_passed} |",
        f"| Elapsed seconds | {blind.elapsed_s:.2f} | {axon.elapsed_s:.2f} |",
        f"| Tool helpers spawned | {blind.actual_tool_helpers} | {axon.actual_tool_helpers} |",
        f"| Max check concurrency | {blind.max_check_concurrency} | {axon.max_check_concurrency} |",
        f"| Subprocesses started | {blind.subprocesses_started} | {axon.subprocesses_started} |",
        f"| App output hash | `{blind.app_output_hash}` | `{axon.app_output_hash}` |",
        f"| App runtime benchmark ms | {blind.app_benchmark_ms:.2f} | {axon.app_benchmark_ms:.2f} |",
        "",
        "## Value",
        "",
        f"- Tool helpers avoided: `{comparison['tool_helpers_avoided']}`",
        f"- Subprocesses avoided: `{comparison['subprocesses_avoided']}`",
        f"- Parallelism reduction: `{comparison['parallelism_reduction']}`",
        f"- Elapsed delta with Axon: `{comparison['elapsed_delta_s']}s`",
        f"- Estimated credits saved: `{comparison['estimated_credits_saved']}`",
        f"- Estimated risk minutes avoided: `{comparison['estimated_risk_minutes_avoided']}`",
        f"- App output identical: `{comparison['app_output_identical']}`",
        f"- App benchmark delta with Axon: `{comparison['app_benchmark_delta_ms']}ms`",
        f"- Tradeoff: {comparison['tradeoff']}",
        f"- Interpretation: {comparison['interpretation']}",
        "",
        "## No-Degradation Fast Path",
        "",
        "This separate lightweight app edit/smoke-check path shows Axon does not have to slow useful work.",
        "",
        "| Metric | Blind agent | Axon-informed agent |",
        "| --- | ---: | ---: |",
        f"| Smoke checks passed | {fast_path.tests_passed} | {fast_path.tests_passed} |",
        f"| Total elapsed seconds | {fast_path.blind_elapsed_s:.2f} | {fast_path.axon_elapsed_s:.2f} |",
        f"| Useful validation seconds | {fast_path.blind_validation_s:.2f} | {fast_path.axon_validation_s:.2f} |",
        f"| Tool helpers spawned | {fast_path.blind_tool_helpers} | {fast_path.axon_tool_helpers} |",
        "",
        f"- Fast-path elapsed delta with Axon: `{fast_delta:.3f}s`",
        f"- Useful validation delta with Axon: `{validation_delta:.3f}s`",
        (
            "- Fast-path interpretation: Axon avoided unnecessary tool setup without degrading useful app validation performance; the remaining overhead is preflight/decision time."
            if validation_delta <= 0.5
            else "- Fast-path interpretation: Axon avoided tool setup, but useful validation was slower in this run."
        ),
        "",
        "## Axon Decision Context",
        "",
        f"- Recommendation: `{axon.axon_recommendation}`",
        f"- Safe parallelism: `{axon.axon_safe_parallelism}`",
        f"- MCP before/after: `{axon.mcp_before}` -> `{axon.mcp_after}`",
        f"- Stale MCP before: `{axon.stale_mcp_before}`",
    ]
    if axon.workflow_impacts:
        lines.append(f"- Workflow impacts: `{', '.join(axon.workflow_impacts)}`")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    axon_bin = sys.argv[1] if len(sys.argv) > 1 else "target/debug/axon"
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else Path("app_dev_ab_report.md")
    if not Path(axon_bin).exists() and shutil.which(axon_bin) is None:
        print(f"[err] axon binary not found: {axon_bin}", file=sys.stderr)
        return 2
    if not shutil.which("node"):
        print("[err] node is required", file=sys.stderr)
        return 2

    blind = run_agent("blind", axon_bin)
    axon = run_agent("axon", axon_bin)
    comparison = compare(blind, axon)
    fast_path = run_fast_path(axon_bin)

    report = render_report(blind, axon, comparison, fast_path)
    out.write_text(report + "\n", encoding="utf-8")
    print(report)
    print(f"[ok] wrote report to {out}")
    return 0 if comparison["both_completed"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
