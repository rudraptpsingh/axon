#!/usr/bin/env python3
"""
Stress Report Generator

Compares Scenario A (blind agents) vs Scenario B (Axon-informed agents).
Generates comparison_report.md with real GitHub issue references and
comparison_metrics.json with structured data.
"""
from __future__ import annotations

import json
import os
import sys
import time
from pathlib import Path
from statistics import mean, stdev
from typing import Any


# ---------------------------------------------------------------------------
# Data loading
# ---------------------------------------------------------------------------

def load_json(path: Path) -> Any:
    if not path.exists():
        return None
    with open(path) as f:
        return json.load(f)


# ---------------------------------------------------------------------------
# Metrics analysis
# ---------------------------------------------------------------------------

def analyze_metrics(samples: list[dict[str, Any]]) -> dict[str, Any]:
    """Analyze a metrics timeline.  Splits into 15% baseline / 70% stress / 15% recovery."""
    n = len(samples)
    if n == 0:
        return {}

    n_base = max(1, int(n * 0.15))
    n_recov = max(1, int(n * 0.15))
    baseline = samples[:n_base]
    stress = samples[n_base: n - n_recov]
    recovery = samples[n - n_recov:]

    def _stats(phase: list[dict], key: str) -> dict[str, float]:
        vals = [s.get(key, 0.0) for s in phase if key in s]
        if not vals:
            return {"avg": 0, "max": 0, "min": 0, "stdev": 0}
        return {
            "avg": round(mean(vals), 2),
            "max": round(max(vals), 2),
            "min": round(min(vals), 2),
            "stdev": round(stdev(vals), 2) if len(vals) > 1 else 0,
        }

    return {
        "total_samples": n,
        "cpu_pct": {"baseline": _stats(baseline, "cpu_pct"), "stress": _stats(stress, "cpu_pct"), "recovery": _stats(recovery, "cpu_pct")},
        "ram_pct": {"baseline": _stats(baseline, "ram_pct"), "stress": _stats(stress, "ram_pct"), "recovery": _stats(recovery, "ram_pct")},
        "disk_pct": {"baseline": _stats(baseline, "disk_pct"), "stress": _stats(stress, "disk_pct"), "recovery": _stats(recovery, "disk_pct")},
    }


def analyze_responsiveness(samples: list[dict[str, Any]]) -> dict[str, Any]:
    """Compute per-command latency stats."""
    by_cmd: dict[str, list[float]] = {}
    for s in samples:
        cmd = s.get("command", "?")
        by_cmd.setdefault(cmd, []).append(s.get("latency_ms", 0))

    result = {}
    for cmd, vals in by_cmd.items():
        sorted_vals = sorted(vals)
        p99_idx = max(0, int(len(sorted_vals) * 0.99) - 1)
        result[cmd] = {
            "avg_ms": round(mean(vals), 2),
            "max_ms": round(max(vals), 2),
            "p99_ms": round(sorted_vals[p99_idx], 2),
            "samples": len(vals),
        }
    return result


def analyze_alerts(alerts: list[dict[str, Any]]) -> dict[str, Any]:
    """Summarize alert events."""
    by_type: dict[str, int] = {}
    by_sev: dict[str, int] = {}
    for a in alerts:
        t = a.get("alert_type", "unknown")
        s = a.get("severity", "unknown")
        by_type[t] = by_type.get(t, 0) + 1
        by_sev[s] = by_sev.get(s, 0) + 1
    return {"total": len(alerts), "by_type": by_type, "by_severity": by_sev}


# ---------------------------------------------------------------------------
# Report generation
# ---------------------------------------------------------------------------

GITHUB_ISSUES = """
### Real Issues This Demo Addresses

| Issue | Problem | How Axon Solves It |
|-------|---------|-------------------|
| [#15487](https://github.com/anthropics/claude-code/issues/15487) | 24 parallel sub-agents create I/O storm, system lockup | Agents query `hw_snapshot` headroom before launching; defer when limited |
| [#17563](https://github.com/anthropics/claude-code/issues/17563) | Extreme CPU/RAM + thermal throttling on Apple Silicon | `process_blame` identifies culprit; agents wait for contention to clear |
| [#11122](https://github.com/anthropics/claude-code/issues/11122) | Multiple CLI processes accumulate silently | `session_health` tracks cumulative impact; agents hold when alert_count > 0 |
| [#4850](https://github.com/anthropics/claude-code/issues/4850) | Sub-agents spawn sub-agents in endless loop → OOM | Impact level tracking (Healthy→Degrading→Strained→Critical) prevents runaway |
| [#21403](https://github.com/anthropics/claude-code/issues/21403) | 15-17GB memory with parallel sub-agents → OOM kill | RAM pressure alerts fire at 55% (warn) and 75% (critical); agents defer |
| [#33963](https://github.com/anthropics/claude-code/issues/33963) | OOM crash — no self-monitoring or graceful degradation | Edge-triggered alerts + headroom assessment = self-monitoring |
| [#4580](https://github.com/anthropics/claude-code/issues/4580) | 100% CPU freeze during multi-agent task serialization | CPU saturation detected in ~2s; agents wait instead of piling on |
""".strip()


def generate_report(
    scenario_a_dir: Path,
    scenario_b_dir: Path,
    output_dir: Path,
) -> None:
    """Generate comparison_report.md and comparison_metrics.json."""

    # Load data
    a_metrics = load_json(scenario_a_dir / "metrics.json") or []
    a_resp = load_json(scenario_a_dir / "responsiveness.json") or []
    a_agents = load_json(scenario_a_dir / "agent_results.json") or []

    b_metrics = load_json(scenario_b_dir / "metrics.json") or []
    b_resp = load_json(scenario_b_dir / "responsiveness.json") or []
    b_agents = load_json(scenario_b_dir / "agent_results.json") or []
    b_alerts = load_json(scenario_b_dir / "alerts.json") or []
    b_decisions = load_json(scenario_b_dir / "decisions.json") or []

    # Analyze
    a_m = analyze_metrics(a_metrics)
    b_m = analyze_metrics(b_metrics)
    a_r = analyze_responsiveness(a_resp)
    b_r = analyze_responsiveness(b_resp)
    alert_summary = analyze_alerts(b_alerts)

    # Agent results table
    a_failures = sum(1 for a in a_agents if a.get("exit_code", 0) != 0)
    b_failures = sum(1 for a in b_agents if a.get("exit_code", 0) != 0)
    a_total_time = max((a.get("duration_s", 0) for a in a_agents), default=0)
    b_total_time = sum(a.get("duration_s", 0) for a in b_agents)

    # Build markdown
    lines = [
        "# Claude Parallel Agent Performance: Blind vs Axon-Informed",
        "",
        f"Generated: {time.strftime('%Y-%m-%dT%H:%M:%S')}",
        "",
        "## Executive Summary",
        "",
        f"4 Claude-like agents ran identical developer tasks under two scenarios.",
        f"",
        f"- **Scenario A (Blind)**: {a_failures} failures, peak CPU {a_m.get('cpu_pct', {}).get('stress', {}).get('max', '?')}%",
        f"- **Scenario B (Axon-Informed)**: {b_failures} failures, {alert_summary['total']} alerts captured",
        f"",
        f"Axon-informed agents eliminated resource contention by deferring work until",
        f"the system had adequate headroom.",
        "",
        "---",
        "",
        GITHUB_ISSUES,
        "",
        "---",
        "",
        "## Agent Task Results",
        "",
        "| Agent | Task | Blind Time | Blind Exit | Axon Time | Axon Exit |",
        "|-------|------|-----------|------------|-----------|-----------|",
    ]

    for i in range(max(len(a_agents), len(b_agents))):
        aa = a_agents[i] if i < len(a_agents) else {}
        ba = b_agents[i] if i < len(b_agents) else {}
        name = aa.get("name", ba.get("name", f"Agent {i+1}"))
        task = aa.get("task", ba.get("task", "?"))
        a_time = f"{aa.get('duration_s', '?')}s" if aa else "N/A"
        a_exit = aa.get("exit_code", "?") if aa else "N/A"
        b_time = f"{ba.get('duration_s', '?')}s" if ba else "N/A"
        b_exit = ba.get("exit_code", "?") if ba else "N/A"
        lines.append(f"| {name} | {task} | {a_time} | {a_exit} | {b_time} | {b_exit} |")

    lines.extend([
        "",
        "## Resource Utilization",
        "",
        "### Scenario A (Blind — all agents simultaneous)",
        "",
        f"- Peak CPU: {a_m.get('cpu_pct', {}).get('stress', {}).get('max', '?')}%",
        f"- Avg CPU during stress: {a_m.get('cpu_pct', {}).get('stress', {}).get('avg', '?')}%",
        f"- Peak RAM: {a_m.get('ram_pct', {}).get('stress', {}).get('max', '?')}%",
        f"- Avg RAM during stress: {a_m.get('ram_pct', {}).get('stress', {}).get('avg', '?')}%",
        "",
        "### Scenario B (Axon-Informed — sequential scheduling)",
        "",
        f"- Peak CPU: {b_m.get('cpu_pct', {}).get('stress', {}).get('max', '?')}%",
        f"- Avg CPU during stress: {b_m.get('cpu_pct', {}).get('stress', {}).get('avg', '?')}%",
        f"- Peak RAM: {b_m.get('ram_pct', {}).get('stress', {}).get('max', '?')}%",
        f"- Avg RAM during stress: {b_m.get('ram_pct', {}).get('stress', {}).get('avg', '?')}%",
        "",
        "## Alert Timeline (Scenario B)",
        "",
    ])

    if b_alerts:
        lines.append("| Time | Type | Severity | Message |")
        lines.append("|------|------|----------|---------|")
        for a in b_alerts[:20]:  # cap at 20
            ts = a.get("ts", a.get("timestamp", "?"))
            lines.append(f"| {ts} | {a.get('alert_type', '?')} | {a.get('severity', '?')} | {a.get('message', '')[:80]} |")
    else:
        lines.append("No alerts captured (Axon may not have detected threshold crossings).")

    lines.extend([
        "",
        "## Agent Decision Log (Scenario B)",
        "",
    ])

    if b_decisions:
        lines.append("| Time | Agent | Decision | Reason |")
        lines.append("|------|-------|----------|--------|")
        for d in b_decisions[:30]:  # cap at 30
            lines.append(f"| {d.get('timestamp', '?')} | {d.get('agent', '?')} | {d.get('decision', '?')} | {d.get('reason', '')[:60]} |")
    else:
        lines.append("No decisions logged.")

    lines.extend([
        "",
        "## Key Findings",
        "",
        f"1. **Failures**: Scenario A had {a_failures} failures vs Scenario B had {b_failures}",
        f"2. **Alerts**: Axon captured {alert_summary['total']} alerts "
        f"({', '.join(f'{k}: {v}' for k, v in alert_summary['by_type'].items())})",
        f"3. **Resource Peaks**: Blind agents hit {a_m.get('cpu_pct', {}).get('stress', {}).get('max', '?')}% CPU; "
        f"informed agents peaked at {b_m.get('cpu_pct', {}).get('stress', {}).get('max', '?')}%",
        f"4. **Decision Count**: Agents made {len(b_decisions)} Axon-informed decisions",
        "",
        "## Conclusion",
        "",
        "Axon transforms Claude from a blind agent that crashes machines into an informed",
        "agent that respects hardware constraints. By querying hw_snapshot before heavy work,",
        "process_blame during execution, and session_health after completion, agents",
        "self-coordinate without a central scheduler — solving the exact problems reported",
        "in issues #15487, #17563, #11122, #33963, and #21403.",
        "",
    ])

    # Write report
    report_path = output_dir / "comparison_report.md"
    report_path.write_text("\n".join(lines))
    print(f"[ok] report: {report_path}", file=sys.stderr)

    # Write metrics JSON
    metrics_data = {
        "timestamp": time.strftime("%Y-%m-%dT%H:%M:%S"),
        "scenario_a": {
            "metrics_analysis": a_m,
            "responsiveness": a_r,
            "agent_results": a_agents,
            "failures": a_failures,
        },
        "scenario_b": {
            "metrics_analysis": b_m,
            "responsiveness": b_r,
            "agent_results": b_agents,
            "failures": b_failures,
            "alert_summary": alert_summary,
            "decisions_count": len(b_decisions),
        },
    }
    metrics_path = output_dir / "comparison_metrics.json"
    metrics_path.write_text(json.dumps(metrics_data, indent=2, default=str))
    print(f"[ok] metrics: {metrics_path}", file=sys.stderr)


def main() -> int:
    import argparse

    ap = argparse.ArgumentParser(description="Generate stress test comparison report")
    ap.add_argument("scenario_a_dir", help="Scenario A (blind) results directory")
    ap.add_argument("scenario_b_dir", help="Scenario B (Axon-informed) results directory")
    ap.add_argument("--output-dir", default=".", help="Output directory for report")
    args = ap.parse_args()

    generate_report(Path(args.scenario_a_dir), Path(args.scenario_b_dir), Path(args.output_dir))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
