#!/usr/bin/env python3
"""
CPU Stress Evaluation for Axon Agent.

Phases:
  1. Baseline      – 12s quiescent, take hw_snapshot + process_blame
  2. CPU Stress    – spawn N `yes > /dev/null` workers, observe for 30s
  3. Peak Capture  – take hw_snapshot + process_blame at peak load
  4. Cooldown      – kill stress, wait 12s, capture recovery state
  5. Session Health – query session_health for the entire window
  6. Hardware Trend – query hardware_trend for last_1h
  7. GPU Snapshot   – query gpu_snapshot
  8. Webhook Alerts – validate real webhook payloads delivered during eval
  9. Serve lifecycle – verify process stayed alive, exits cleanly

Usage: python3 scripts/eval_cpu_stress.py ./target/release/axon
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any

AXON = sys.argv[1] if len(sys.argv) > 1 else "./target/release/axon"
# Scale workers to push system CPU above the 72% saturation alert threshold.
# Each `yes` process saturates one core; we need ~75% of cores busy to
# reliably cross the threshold even with background load fluctuation.
_cpu_count = os.cpu_count() or 4
NUM_STRESS_WORKERS = max(4, int(_cpu_count * 0.80))
MSG_ID = 0


# ── Embedded webhook receiver ──────────────────────────────────────

class WebhookCollector:
    """Thread-safe collector that runs an HTTP server to receive alert webhooks."""

    def __init__(self) -> None:
        self._alerts: list[dict[str, Any]] = []
        self._lock = threading.Lock()
        self._server: HTTPServer | None = None
        self._thread: threading.Thread | None = None
        self.port: int = 0
        self.url: str = ""

    def start(self) -> str:
        """Start the receiver on a random port. Returns the webhook URL."""
        collector = self

        class Handler(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                length = int(self.headers.get("Content-Length", 0))
                raw = self.rfile.read(length)
                try:
                    obj = json.loads(raw)
                    with collector._lock:
                        collector._alerts.append(obj)
                except json.JSONDecodeError:
                    pass
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"ok\n")

            def log_message(self, *args: object) -> None:
                pass  # suppress HTTP logs

        self._server = HTTPServer(("127.0.0.1", 0), Handler)
        self.port = self._server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}/alerts"
        self._thread = threading.Thread(target=self._server.serve_forever, daemon=True)
        self._thread.start()
        return self.url

    def stop(self) -> None:
        if self._server:
            self._server.shutdown()

    def get_alerts(self) -> list[dict[str, Any]]:
        with self._lock:
            return list(self._alerts)


# ── MCP JSON-RPC helpers ───────────────────────────────────────────

def next_id() -> int:
    global MSG_ID
    MSG_ID += 1
    return MSG_ID


def send(proc: subprocess.Popen, obj: dict[str, Any]) -> None:
    assert proc.stdin
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def read_responses(proc: subprocess.Popen, until_id: int, timeout_s: float = 45.0) -> dict[str, Any]:
    buf: list[dict[str, Any]] = []
    deadline = time.time() + timeout_s
    assert proc.stdout
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        buf.append(msg)
        if msg.get("id") == until_id and "result" in msg:
            return msg
    raise RuntimeError(f"timeout waiting for id={until_id}; last msgs: {buf[-3:]!r}")


def call_tool(proc: subprocess.Popen, name: str, args: dict | None = None) -> dict[str, Any]:
    mid = next_id()
    send(proc, {
        "jsonrpc": "2.0",
        "id": mid,
        "method": "tools/call",
        "params": {"name": name, "arguments": args or {}},
    })
    res = read_responses(proc, mid)
    content = res["result"]["content"]
    text = content[0]["text"]
    return json.loads(text)


def list_tools(proc: subprocess.Popen) -> list[str]:
    mid = next_id()
    send(proc, {"jsonrpc": "2.0", "id": mid, "method": "tools/list", "params": {}})
    res = read_responses(proc, mid)
    tools = res.get("result", {}).get("tools", [])
    return [t["name"] for t in tools]


def initialize_mcp(proc: subprocess.Popen) -> None:
    mid = next_id()
    send(proc, {
        "jsonrpc": "2.0",
        "id": mid,
        "method": "initialize",
        "params": {
            "protocolVersion": "2025-06-18",
            "capabilities": {},
            "clientInfo": {"name": "cpu-stress-eval", "version": "0.2.0"},
        },
    })
    send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
    read_responses(proc, mid)


# ── Display helpers ────────────────────────────────────────────────

HEADROOM_ORDER = {"adequate": 2, "limited": 1, "insufficient": 0}


def headroom_ge(a: str, b: str) -> bool:
    """Return True if headroom level `a` is at least as good as `b`."""
    return HEADROOM_ORDER.get(a, -1) >= HEADROOM_ORDER.get(b, -1)


def print_snapshot(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n{'=' * 60}")
    print(f"  {label}")
    print(f"{'=' * 60}")
    print(f"  CPU:       {d.get('cpu_usage_pct', 'N/A'):.1f}%")
    print(f"  RAM:       {d.get('ram_used_gb', 0):.2f} / {d.get('ram_total_gb', 0):.1f} GB")
    print(f"  RAM press: {d.get('ram_pressure', 'N/A')}")
    disk_used = d.get('disk_used_gb', 0)
    disk_total = d.get('disk_total_gb', 0)
    disk_press = d.get('disk_pressure', 'N/A')
    print(f"  Disk:      {disk_used:.1f} / {disk_total:.1f} GB ({disk_press})")
    temp = d.get('die_temp_celsius')
    print(f"  Die temp:  {f'{temp:.1f}C' if temp else 'N/A'}")
    print(f"  Throttle:  {d.get('throttling', False)}")
    print(f"  Headroom:  {d.get('headroom', 'N/A')}")
    print(f"  Reason:    {d.get('headroom_reason', '')}")
    print(f"  Narrative: {data.get('narrative', '')}")


def print_blame(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n--- {label} ---")
    print(f"  Anomaly:   {d.get('anomaly_type', 'N/A')}")
    print(f"  Impact:    {d.get('impact_level', 'N/A')}")
    print(f"  Score:     {d.get('anomaly_score', 0):.3f}")
    c = d.get("culprit")
    if c:
        print(f"  Culprit:   {c.get('cmd', '?')} (PID {c.get('pid')}, CPU {c.get('cpu_pct', 0):.1f}%, RAM {c.get('ram_gb', 0):.2f}GB)")
    g = d.get("culprit_group")
    if g:
        print(f"  Group:     {g.get('name', '?')} ({g.get('process_count')} procs, CPU {g.get('total_cpu_pct', 0):.1f}%, RAM {g.get('total_ram_gb', 0):.2f}GB)")
    print(f"  Impact:    {d.get('impact', '')}")
    print(f"  Fix:       {d.get('fix', '')}")
    print(f"  Narrative: {data.get('narrative', '')}")


def print_session_health(label: str, data: dict) -> None:
    d = data.get("data", data)
    print(f"\n{'=' * 60}")
    print(f"  {label}")
    print(f"{'=' * 60}")
    print(f"  Snapshots:     {d.get('snapshot_count', 0)}")
    print(f"  Alerts:        {d.get('alert_count', 0)}")
    print(f"  Worst Impact:  {d.get('worst_impact_level', 'N/A')}")
    print(f"  Worst Anomaly: {d.get('worst_anomaly_type', 'N/A')}")
    print(f"  Avg CPU:       {d.get('avg_cpu_pct', 0):.1f}%")
    print(f"  Peak CPU:      {d.get('peak_cpu_pct', 0):.1f}%")
    print(f"  Avg RAM:       {d.get('avg_ram_gb', 0):.2f} GB")
    print(f"  Peak RAM:      {d.get('peak_ram_gb', 0):.2f} GB")
    print(f"  Throttle evts: {d.get('throttle_event_count', 0)}")
    print(f"  Narrative:     {data.get('narrative', '')}")


# ── Main ───────────────────────────────────────────────────────────

def main() -> int:
    print("[info] CPU Stress Evaluation for Axon")
    print(f"[info] Binary: {AXON}")
    print(f"[info] Stress workers: {NUM_STRESS_WORKERS}")

    # ── Start webhook receiver ──
    webhook = WebhookCollector()
    webhook_url = webhook.start()
    print(f"[ok] Webhook receiver listening on {webhook_url}")

    # Record session start for session_health query
    session_start = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())

    # Start axon serve with webhook pointed at our receiver (accept all alerts)
    proc = subprocess.Popen(
        [
            AXON, "serve",
            "--alert-webhook", f"eval={webhook_url}",
        ],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.PIPE,
        text=True,
        bufsize=1,
    )

    stress_procs: list[subprocess.Popen] = []

    try:
        initialize_mcp(proc)
        print("[ok] MCP initialized")

        # ── Verify all 7 MCP tools are registered ──
        tool_names = list_tools(proc)
        print(f"[info] MCP tools ({len(tool_names)}): {tool_names}")
        expected_tools = {"hw_snapshot", "process_blame", "battery_status",
                          "system_profile", "hardware_trend", "session_health",
                          "gpu_snapshot"}
        missing = expected_tools - set(tool_names)
        if missing:
            print(f"[FAIL] Missing MCP tools: {missing}")
            return 1
        print(f"[ok] All 7 MCP tools registered")

        # ── Phase 0: Stale Instance Detection ──
        print("\n[phase 0] Stale instance detection...")
        profile = call_tool(proc, "system_profile")
        profile_d = profile.get("data", {})
        startup_warnings = profile_d.get("startup_warnings", [])
        print(f"  Startup warnings: {len(startup_warnings)}")
        for w in startup_warnings:
            print(f"    [WARN] {w}")
        print(f"  Narrative: {profile.get('narrative', '')}")

        # Wait for collector warm-up before checking blame
        print("[info] Waiting 8s for collector warm-up before blame check...")
        time.sleep(8)

        initial_blame = call_tool(proc, "process_blame")
        initial_blame_d = initial_blame.get("data", {})
        stale_axon_pids = initial_blame_d.get("stale_axon_pids", [])
        print(f"  Stale axon PIDs: {stale_axon_pids}")
        if stale_axon_pids:
            print(f"  Fix: {initial_blame_d.get('fix', '')}")
            print(f"  Narrative: {initial_blame.get('narrative', '')}")

        # ── Phase 1: Baseline (collector already warmed 8s from phase 0) ──
        print("\n[phase 1] Baseline -- waiting 4s for remaining EWMA warmup...")
        time.sleep(4)

        baseline_hw = call_tool(proc, "hw_snapshot")
        baseline_blame = call_tool(proc, "process_blame")
        print_snapshot("BASELINE hw_snapshot", baseline_hw)
        print_blame("BASELINE process_blame", baseline_blame)

        # Snapshot webhook count after baseline (before stress)
        pre_stress_webhook_count = len(webhook.get_alerts())
        print(f"[info] Webhooks received during baseline: {pre_stress_webhook_count}")

        # ── Phase 2: CPU Stress ──
        print(f"\n[phase 2] Starting {NUM_STRESS_WORKERS} CPU stress workers (yes > /dev/null)...")
        for i in range(NUM_STRESS_WORKERS):
            p = subprocess.Popen(
                ["yes"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            stress_procs.append(p)
            print(f"  [info] Worker {i+1} started (PID {p.pid})")

        # Let stress build up and axon detect it (need several ticks)
        print("[info] Waiting 16s for axon to detect CPU saturation...")
        time.sleep(16)

        # Take snapshots during stress
        stress_hw_1 = call_tool(proc, "hw_snapshot")
        stress_blame_1 = call_tool(proc, "process_blame")
        print_snapshot("STRESS hw_snapshot (t+16s)", stress_hw_1)
        print_blame("STRESS process_blame (t+16s)", stress_blame_1)

        # Wait more and take another reading
        print("[info] Waiting 14s more for sustained detection...")
        time.sleep(14)

        stress_hw_2 = call_tool(proc, "hw_snapshot")
        stress_blame_2 = call_tool(proc, "process_blame")
        print_snapshot("STRESS hw_snapshot (t+30s)", stress_hw_2)
        print_blame("STRESS process_blame (t+30s)", stress_blame_2)

        # ── Phase 3: Kill stress, observe recovery ──
        print(f"\n[phase 3] Killing stress workers...")
        for p in stress_procs:
            p.kill()
            p.wait()
        stress_procs.clear()
        print("[info] All stress workers killed. Waiting 12s for recovery...")
        time.sleep(12)

        recovery_hw = call_tool(proc, "hw_snapshot")
        recovery_blame = call_tool(proc, "process_blame")
        print_snapshot("RECOVERY hw_snapshot (12s post-stress)", recovery_hw)
        print_blame("RECOVERY process_blame (12s post-stress)", recovery_blame)

        # ── Phase 4: Session Health ──
        print(f"\n[phase 4] Querying session_health since {session_start}...")
        session = call_tool(proc, "session_health", {"since": session_start})
        print_session_health("SESSION HEALTH (entire evaluation)", session)

        # ── Phase 5: Hardware Trend ──
        print(f"\n[phase 5] Querying hardware_trend (last_1h, 1m buckets)...")
        trend = call_tool(proc, "hardware_trend", {"time_range": "last_1h", "interval": "1m"})
        td = trend.get("data", {})
        buckets = td.get("buckets", [])
        print(f"  Trend direction: {td.get('trend_direction', 'N/A')}")
        print(f"  Total snapshots: {td.get('total_snapshots', 0)}")
        print(f"  Buckets: {len(buckets)}")
        for b in buckets:
            ts = b.get("bucket_start", "?")[-8:]  # just time part
            print(f"    {ts}: CPU avg={b['cpu_avg']:.1f}% max={b['cpu_max']:.1f}% | RAM avg={b['ram_avg']:.2f}GB max={b['ram_max']:.2f}GB | anomalies={b['anomaly_count']} throttles={b['throttle_count']}")

        # ── Phase 6: GPU Snapshot ──
        print(f"\n[phase 6] Querying gpu_snapshot...")
        gpu = call_tool(proc, "gpu_snapshot")
        gpu_d = gpu.get("data", {})
        print(f"  OK:           {gpu.get('ok', False)}")
        print(f"  Detected:     {gpu_d.get('detected', False)}")
        print(f"  Model:        {gpu_d.get('model', 'N/A')}")
        print(f"  Utilization:  {gpu_d.get('utilization_pct', 'N/A')}")
        print(f"  Narrative:    {gpu.get('narrative', '')}")

        # ── Phase 7: Webhook Alert Validation ──
        print(f"\n[phase 7] Validating webhook alert payloads...")
        all_webhooks = webhook.get_alerts()
        print(f"  Total webhooks received: {len(all_webhooks)}")

        # Print each webhook payload summary
        VALID_SEVERITIES = {"warning", "critical", "resolved"}
        VALID_TYPES = {"memory_pressure", "thermal_throttle", "disk_pressure",
                       "cpu_saturation", "impact_escalation"}
        REQUIRED_PAYLOAD_KEYS = {"alert_type", "severity", "resolved", "timestamp",
                                 "message", "metrics"}

        malformed_webhooks: list[str] = []
        severity_set: set[str] = set()
        type_set: set[str] = set()
        has_metrics = True
        has_culprit_in_any = False

        for i, wh in enumerate(all_webhooks):
            # Structural validation
            missing_keys = REQUIRED_PAYLOAD_KEYS - set(wh.keys())
            if missing_keys:
                malformed_webhooks.append(f"webhook[{i}]: missing keys {missing_keys}")
                continue

            sev = wh.get("severity", "")
            atype = wh.get("alert_type", "")
            severity_set.add(sev)
            type_set.add(atype)

            if sev not in VALID_SEVERITIES:
                malformed_webhooks.append(f"webhook[{i}]: invalid severity '{sev}'")
            if atype not in VALID_TYPES:
                malformed_webhooks.append(f"webhook[{i}]: invalid alert_type '{atype}'")

            # resolved field must match severity
            resolved_flag = wh.get("resolved", None)
            if resolved_flag != (sev == "resolved"):
                malformed_webhooks.append(
                    f"webhook[{i}]: resolved={resolved_flag} but severity='{sev}'")

            # metrics must have numeric fields
            metrics = wh.get("metrics", {})
            if not isinstance(metrics, dict):
                has_metrics = False
            else:
                # At least one metric should be non-null
                metric_vals = [metrics.get(k) for k in ("ram_pct", "cpu_pct", "temp_c", "disk_pct")]
                if all(v is None for v in metric_vals):
                    has_metrics = False

            if wh.get("culprit") is not None:
                has_culprit_in_any = True

            # Print compact summary for first 10 and last 2
            if i < 10 or i >= len(all_webhooks) - 2:
                ts_short = wh.get("timestamp", "?")[-12:]
                culprit_name = wh.get("culprit", {}).get("name", "-") if wh.get("culprit") else "-"
                cpu = metrics.get("cpu_pct")
                ram = metrics.get("ram_pct")
                print(f"    [{i:2d}] {sev:<9s} {atype:<20s} cpu={cpu or '-':>5} ram={ram or '-':>5} culprit={culprit_name} @{ts_short}")
            elif i == 10:
                print(f"    ... ({len(all_webhooks) - 12} more) ...")

        if malformed_webhooks:
            print(f"\n  MALFORMED WEBHOOKS:")
            for m in malformed_webhooks[:10]:
                print(f"    [warn] {m}")

        print(f"\n  Webhook summary:")
        print(f"    Severities seen:   {sorted(severity_set) if severity_set else 'none'}")
        print(f"    Alert types seen:  {sorted(type_set) if type_set else 'none'}")
        print(f"    Has metrics:       {has_metrics}")
        print(f"    Has culprit:       {has_culprit_in_any}")
        print(f"    Malformed count:   {len(malformed_webhooks)}")

        # ── Phase 8: Serve Lifecycle Check ──
        print(f"\n[phase 8] Checking serve process lifecycle...")
        serve_alive = proc.poll() is None
        print(f"  Serve alive:  {serve_alive} (pid={proc.pid})")

        # ── Summary Analysis ──
        print(f"\n{'=' * 60}")
        print("  EVALUATION SUMMARY")
        print(f"{'=' * 60}")

        b_data = baseline_hw.get("data", {})
        s_data = stress_hw_2.get("data", {})
        r_data = recovery_hw.get("data", {})

        b_cpu = b_data.get("cpu_usage_pct", 0)
        s_cpu = s_data.get("cpu_usage_pct", 0)
        r_cpu = r_data.get("cpu_usage_pct", 0)
        print(f"  CPU baseline:  {b_cpu:.1f}%")
        print(f"  CPU peak:      {s_cpu:.1f}%")
        print(f"  CPU recovery:  {r_cpu:.1f}%")

        b_headroom = b_data.get("headroom", "?")
        s_headroom = s_data.get("headroom", "?")
        r_headroom = r_data.get("headroom", "?")
        print(f"  Headroom:      {b_headroom} -> {s_headroom} -> {r_headroom}")

        b_anomaly = baseline_blame.get("data", {}).get("anomaly_type", "?")
        s_anomaly = stress_blame_2.get("data", {}).get("anomaly_type", "?")
        r_anomaly = recovery_blame.get("data", {}).get("anomaly_type", "?")
        print(f"  Anomaly:       {b_anomaly} -> {s_anomaly} -> {r_anomaly}")

        b_impact = baseline_blame.get("data", {}).get("impact_level", "?")
        s_impact = stress_blame_2.get("data", {}).get("impact_level", "?")
        r_impact = recovery_blame.get("data", {}).get("impact_level", "?")
        print(f"  Impact:        {b_impact} -> {s_impact} -> {r_impact}")

        s_alert_count = session.get("data", {}).get("alert_count", 0)
        print(f"  Alerts (DB):   {s_alert_count}")
        print(f"  Webhooks:      {len(all_webhooks)}")

        # ── Pre-existing pressure ──
        b_ram_press = b_data.get("ram_pressure", "normal")
        b_disk_press = b_data.get("disk_pressure", "normal")
        b_throttle = b_data.get("throttling", False)
        pre_existing: list[str] = []
        if b_ram_press != "normal":
            ram_pct = b_data.get("ram_used_gb", 0) / max(b_data.get("ram_total_gb", 1), 0.01) * 100
            pre_existing.append(f"RAM {b_ram_press} ({ram_pct:.0f}%)")
        if b_disk_press != "normal":
            disk_pct = b_data.get("disk_used_gb", 0) / max(b_data.get("disk_total_gb", 1), 0.01) * 100
            pre_existing.append(f"Disk {b_disk_press} ({disk_pct:.0f}%)")
        if b_throttle:
            pre_existing.append(f"Throttling ({b_data.get('die_temp_celsius', '?')}C)")

        if pre_existing:
            print(f"\n  PRE-EXISTING PRESSURE:")
            for p in pre_existing:
                print(f"    [info] {p}")
            print(f"    [info] Baseline headroom already {b_headroom} before CPU stress")
        else:
            print(f"\n  PRE-EXISTING PRESSURE: none")

        # ── Checks ──
        checks: list[tuple[str, bool, str]] = []

        # -- CPU signal checks --
        detected_stress = s_cpu > b_cpu + 20
        checks.append(("CPU spike detected", detected_stress,
                        f"{b_cpu:.0f}% -> {s_cpu:.0f}%"))

        recovered = r_cpu < s_cpu - 10
        checks.append(("CPU recovered after kill", recovered,
                        f"{s_cpu:.0f}% -> {r_cpu:.0f}%"))

        # -- Headroom checks --
        headroom_degraded = s_headroom == "insufficient"
        checks.append(("Headroom insufficient during stress", headroom_degraded,
                        f"{s_headroom}"))

        headroom_recovered = headroom_ge(r_headroom, b_headroom)
        checks.append(("Headroom recovered to baseline level", headroom_recovered,
                        f"{s_headroom} -> {r_headroom} (baseline was {b_headroom})"))

        # -- Anomaly checks --
        anomaly_detected = s_anomaly != "none"
        checks.append(("Anomaly detected during stress", anomaly_detected,
                        f"{s_anomaly}"))

        anomaly_cleared = r_anomaly != "cpu_saturation"
        checks.append(("CPU saturation cleared after recovery", anomaly_cleared,
                        f"{r_anomaly}"))

        # -- Alert DB checks --
        alerts_fired = s_alert_count > 0
        checks.append(("Alerts persisted to DB", alerts_fired,
                        f"alert_count={s_alert_count}"))

        # -- Webhook delivery checks --
        webhook_received = len(all_webhooks) > 0
        checks.append(("Webhook alerts delivered", webhook_received,
                        f"received={len(all_webhooks)}"))

        webhook_well_formed = len(malformed_webhooks) == 0
        checks.append(("Webhook payloads well-formed", webhook_well_formed,
                        f"malformed={len(malformed_webhooks)}"))

        webhook_has_metrics = has_metrics
        checks.append(("Webhook payloads include metrics", webhook_has_metrics,
                        f"has_metrics={has_metrics}"))

        # Webhook severities should be valid edge-trigger values
        webhook_valid_severities = severity_set <= VALID_SEVERITIES
        checks.append(("Webhook severities are valid", webhook_valid_severities,
                        f"seen={sorted(severity_set)}"))

        # Webhook alert_types should be valid
        webhook_valid_types = type_set <= VALID_TYPES
        checks.append(("Webhook alert_types are valid", webhook_valid_types,
                        f"seen={sorted(type_set)}"))

        # -- Stale instance detection checks --
        profile_has_warnings_field = isinstance(startup_warnings, list)
        checks.append(("system_profile has startup_warnings field",
                        profile_has_warnings_field,
                        f"type={type(startup_warnings).__name__}"))

        blame_has_stale_field = isinstance(stale_axon_pids, list)
        checks.append(("process_blame has stale_axon_pids field",
                        blame_has_stale_field,
                        f"type={type(stale_axon_pids).__name__}"))

        # Self-PID exclusion: the eval's own axon serve should NOT appear
        # in the blame culprit (since we exclude self_pid)
        blame_culprit_pid = initial_blame_d.get("culprit", {}).get("pid") if initial_blame_d.get("culprit") else None
        self_not_in_blame = blame_culprit_pid != proc.pid
        checks.append(("Self PID excluded from blame",
                        self_not_in_blame,
                        f"culprit_pid={blame_culprit_pid}, serve_pid={proc.pid}"))

        # -- GPU check --
        gpu_valid = gpu.get("ok") is True
        checks.append(("GPU snapshot returns valid JSON", gpu_valid,
                        f"ok={gpu.get('ok')}"))

        # -- Serve lifecycle --
        checks.append(("Serve process alive throughout eval", serve_alive,
                        f"pid={proc.pid}"))

        print(f"\n  CHECKS:")
        all_pass = True
        for label, passed, detail in checks:
            status = "[pass]" if passed else "[FAIL]"
            if not passed:
                all_pass = False
            print(f"    {status} {label} ({detail})")

        print(f"\n  RESULT: {'ALL CHECKS PASSED' if all_pass else 'SOME CHECKS FAILED'}")
        return 0 if all_pass else 1

    finally:
        # Cleanup stress workers
        for p in stress_procs:
            try:
                p.kill()
                p.wait()
            except Exception:
                pass
        # Stop webhook receiver
        webhook.stop()
        # Test clean exit: close stdin and wait for graceful shutdown
        if proc.stdin and not proc.stdin.closed:
            proc.stdin.close()
        try:
            proc.wait(timeout=5)
            print(f"[ok] Serve exited cleanly on stdin close (rc={proc.returncode})")
        except subprocess.TimeoutExpired:
            print("[warn] Serve did not exit on stdin close, terminating")
            proc.terminate()
            try:
                proc.wait(timeout=3)
            except subprocess.TimeoutExpired:
                proc.kill()


if __name__ == "__main__":
    sys.exit(main())
