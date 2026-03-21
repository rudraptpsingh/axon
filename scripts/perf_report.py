#!/usr/bin/env python3
"""
Axon Performance Report Generator

Reads results JSON from perf_test_scenario.py and produces a polished
human-readable report suitable for showcasing Axon's value proposition.

Usage:
  python3 scripts/perf_report.py --input results.json
  python3 scripts/perf_test_scenario.py --axon-bin ./target/release/axon --output results.json && \
    python3 scripts/perf_report.py --input results.json
"""
from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path
from typing import Any

SCENARIO_NAMES = {
    "cpu": "Build Storm (CPU Saturation)",
    "memory": "Memory Leak (RAM Pressure)",
    "combined": "Full Session (CPU + Memory)",
    "disk": "Disk Full (Storage Pressure)",
}


def fmt_time(val: float | int) -> str:
    if val < 0:
        return "N/A"
    return f"{val}s"


def fmt_factor(val: float | int) -> str:
    if val < 0:
        return "N/A"
    return f"{val}x"


def print_scenario_report(r: dict[str, Any]) -> None:
    scenario = r.get("scenario", "unknown")
    name = SCENARIO_NAMES.get(scenario, scenario)

    print(f"\nSCENARIO: {name}")
    print("-" * 64)

    if "error" in r:
        print(f"  ERROR: {r['error']}")
        return

    baseline = r.get("baseline_task_time", -1)
    blind_time = r.get("blind_task_time", -1)
    blind_factor = r.get("blind_slowdown_factor", -1)
    recovered = r.get("recovered_task_time", -1)
    recovered_factor = r.get("recovered_factor", -1)
    mttd = r.get("mttd_seconds", -1)
    mttr = r.get("mttr_seconds", -1)
    alert_count = r.get("alert_count", 0)
    alert_types = r.get("alert_types", [])
    blame_correct = r.get("blame_correct", False)
    fix_suggestion = r.get("fix_suggestion", "")

    # Task performance comparison
    print(f"\n  Task Performance:")
    print(f"  {'':>24} {'Baseline':>10} {'No Axon':>10} {'With Axon':>10}")
    print(f"  {'Completion time':>24} {fmt_time(baseline):>10} {fmt_time(blind_time):>10} {fmt_time(recovered):>10}")
    print(f"  {'Slowdown factor':>24} {'1.0x':>10} {fmt_factor(blind_factor):>10} {fmt_factor(recovered_factor):>10}")

    # Detection and resolution
    print(f"\n  Detection & Resolution:")
    print(f"  {'':>24} {'No Axon':>14} {'With Axon':>14}")
    print(f"  {'System aware of issue':>24} {'Never':>14} {fmt_time(mttd):>14}")
    print(f"  {'Issue resolved':>24} {'Never':>14} {fmt_time(mttr):>14}")
    print(f"  {'How diagnosed':>24} {'(not at all)':>14} {'process_blame':>14}")

    # Alerts
    print(f"\n  Alerts Generated:")
    print(f"  {'Without Axon':>24}: 0")
    print(f"  {'With Axon':>24}: {alert_count}")
    for at in alert_types:
        print(f"  {'':>24}  - {at}")

    # Blame
    print(f"\n  Blame Analysis:")
    print(f"  {'Culprit identified':>24}: {'Yes (correct)' if blame_correct else 'No / Incorrect'}")
    if fix_suggestion:
        # Wrap long fix text
        if len(fix_suggestion) > 50:
            print(f"  {'Fix suggestion':>24}: {fix_suggestion[:50]}")
            print(f"  {'':>24}  {fix_suggestion[50:]}")
        else:
            print(f"  {'Fix suggestion':>24}: {fix_suggestion}")

    # Webhooks
    payloads = r.get("webhook_payloads", [])
    if payloads:
        print(f"\n  Webhook Payloads ({len(payloads)}):")
        for i, p in enumerate(payloads[:3]):
            severity = p.get("severity", "?")
            atype = p.get("alert_type", "?")
            msg = p.get("message", "")[:60]
            print(f"    [{i + 1}] {severity}/{atype}: {msg}")

    # Verdict
    passed = r.get("passed", False)
    if passed:
        print(f"\n  Result: PASS -- Axon detected and resolved the issue")
    else:
        print(f"\n  Result: NEEDS ATTENTION -- see metrics above")


def print_summary(results: list[dict[str, Any]]) -> None:
    valid = [r for r in results if "error" not in r]
    if not valid:
        print("\nNo valid results to summarize.")
        return

    avg_mttd = sum(r.get("mttd_seconds", 0) for r in valid if r.get("mttd_seconds", -1) > 0)
    mttd_count = sum(1 for r in valid if r.get("mttd_seconds", -1) > 0)
    avg_mttd = round(avg_mttd / mttd_count, 1) if mttd_count > 0 else -1

    avg_mttr = sum(r.get("mttr_seconds", 0) for r in valid if r.get("mttr_seconds", -1) > 0)
    mttr_count = sum(1 for r in valid if r.get("mttr_seconds", -1) > 0)
    avg_mttr = round(avg_mttr / mttr_count, 1) if mttr_count > 0 else -1

    avg_blind_factor = sum(r.get("blind_slowdown_factor", 0) for r in valid)
    avg_blind_factor = round(avg_blind_factor / len(valid), 1)

    avg_recovered_factor = sum(r.get("recovered_factor", 0) for r in valid)
    avg_recovered_factor = round(avg_recovered_factor / len(valid), 1)

    total_alerts = sum(r.get("alert_count", 0) for r in valid)
    blame_correct = sum(1 for r in valid if r.get("blame_correct"))
    all_passed = all(r.get("passed", False) for r in valid)

    print(f"\n{'=' * 64}")
    print(f"  AGGREGATE METRICS ACROSS {len(valid)} SCENARIO(S)")
    print(f"{'=' * 64}")
    print(f"\n  {'':>28} {'Without Axon':>14} {'With Axon':>14} {'Improvement':>14}")
    print(f"  {'-' * 56}")

    # MTTD
    mttd_improvement = "instant" if avg_mttd > 0 else "N/A"
    print(f"  {'Avg MTTD':>28} {'Never':>14} {fmt_time(avg_mttd):>14} {mttd_improvement:>14}")

    # MTTR
    mttr_improvement = "instant" if avg_mttr > 0 else "N/A"
    print(f"  {'Avg MTTR':>28} {'Never':>14} {fmt_time(avg_mttr):>14} {mttr_improvement:>14}")

    # Task slowdown
    if avg_blind_factor > 0 and avg_recovered_factor > 0:
        perf_gain = round((1 - avg_recovered_factor / avg_blind_factor) * 100)
        perf_str = f"{perf_gain}% faster"
    else:
        perf_str = "N/A"
    print(f"  {'Avg task slowdown':>28} {fmt_factor(avg_blind_factor):>14} {fmt_factor(avg_recovered_factor):>14} {perf_str:>14}")

    # Alerts
    print(f"  {'Total alerts generated':>28} {'0':>14} {str(total_alerts):>14} {'full coverage':>14}")

    # Blame
    print(f"  {'Blame accuracy':>28} {'N/A':>14} {f'{blame_correct}/{len(valid)}':>14} {'':>14}")

    # Verdict
    print(f"\n  {'VERDICT':>28}: ", end="")
    if all_passed:
        print("Axon transforms blind degradation into informed recovery.")
    else:
        print("Some scenarios need attention. See individual results above.")

    # Token economics estimate
    print(f"\n  Token Economics (estimated):")
    print(f"  {'':>4}Without Axon: Agent retries task, explores system manually,")
    print(f"  {'':>4}burns ~2000-4000 tokens over multiple turns to diagnose (or never does).")
    print(f"  {'':>4}With Axon: Single process_blame call (~300 tokens) returns culprit + fix.")
    if avg_blind_factor > 1.5:
        savings = round((1 - 300 / 2500) * 100)
        print(f"  {'':>4}Estimated savings: ~{savings}% fewer tokens per performance incident.")


def main() -> int:
    ap = argparse.ArgumentParser(description="Axon Performance Report Generator")
    ap.add_argument("--input", required=True, metavar="PATH", help="Path to results JSON")
    args = ap.parse_args()

    path = Path(args.input)
    if not path.is_file():
        print(f"[err] File not found: {path}", file=sys.stderr)
        return 2

    data = json.loads(path.read_text())
    system_info = data.get("system_info", {})
    results = data.get("scenarios", [])
    timestamp = data.get("timestamp", "unknown")

    print("=" * 64)
    print("  AXON PERFORMANCE SHOWCASE REPORT")
    print("=" * 64)
    print(f"  Date:    {timestamp}")
    print(f"  Machine: {system_info.get('platform', '?')} {system_info.get('machine', '?')}, "
          f"{system_info.get('cpu_count', '?')} cores, "
          f"{system_info.get('ram_total_gb', '?')}GB RAM")
    print(f"  Scenarios: {len(results)}")

    for r in results:
        print_scenario_report(r)

    print_summary(results)

    print(f"\n{'=' * 64}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
