#!/usr/bin/env python3
"""
End-to-end: run axon serve with webhook (via alert-dispatch.json or --alert-webhook),
stress CPU so state can change, then consume alert POSTs and/or verify SQLite rows.

Modes:
  (default)      Write alert-dispatch.json (webhook-only) and set AXON_CONFIG_DIR.
  --cli          Empty config dir + --alert-webhook ID=URL (MCP default from axon + webhook from CLI).
  --mcp-and-webhook  With default mode: prepend mcp channel to JSON (explicit dual routing).

Optional: sqlite3 for DB checks.

Success if at least one valid JSON POST is received on the local webhook.

Usage:
  python3 scripts/test_alert_webhooks_live.py [path/to/axon]
  python3 scripts/test_alert_webhooks_live.py --cli [path/to/axon]
  python3 scripts/test_alert_webhooks_live.py --port 9999 [path/to/axon]
"""
from __future__ import annotations

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path


def sqlite_alert_count() -> int | None:
    """macOS default DB: ~/Library/Application Support/axon/hardware.db"""
    db = Path.home() / "Library/Application Support/axon/hardware.db"
    if not db.is_file():
        return None
    try:
        r = subprocess.run(
            ["sqlite3", str(db), "SELECT COUNT(*) FROM alerts;"],
            capture_output=True,
            text=True,
            timeout=5,
        )
        if r.returncode != 0:
            return None
        return int(r.stdout.strip())
    except (FileNotFoundError, ValueError, subprocess.TimeoutExpired):
        return None


def main() -> int:
    root = Path(__file__).resolve().parent.parent
    ap = argparse.ArgumentParser(description="E2E webhook delivery with real axon serve")
    ap.add_argument(
        "axon",
        nargs="?",
        default=root / "target/release/axon",
        type=Path,
        help="Path to axon binary (default: target/release/axon)",
    )
    ap.add_argument(
        "--cli",
        action="store_true",
        help="Use --alert-webhook instead of alert-dispatch.json",
    )
    ap.add_argument(
        "--port",
        type=int,
        default=0,
        metavar="N",
        help="Bind webhook listener to 127.0.0.1:N (0 = random free port)",
    )
    ap.add_argument(
        "--mcp-and-webhook",
        action="store_true",
        help="Config file includes default MCP channel + webhook (webhook POST still required)",
    )
    args = ap.parse_args()
    use_cli = args.cli

    axon: Path = args.axon
    if not axon.is_file():
        print(f"[err] axon binary not found: {axon}", file=sys.stderr)
        print("  Run: cargo build --release -p axon", file=sys.stderr)
        return 2

    posts: list[str] = []
    lock = threading.Lock()

    class H(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            ln = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(ln)
            with lock:
                posts.append(raw.decode())
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok")

        def log_message(self, *args: object) -> None:
            pass

    bind_port = args.port if args.port > 0 else 0
    srv = HTTPServer(("127.0.0.1", bind_port), H)
    port = srv.server_address[1]
    url = f"http://127.0.0.1:{port}/alerts"
    t = threading.Thread(target=srv.serve_forever, daemon=True)
    t.start()

    cfg_dir = tempfile.mkdtemp(prefix="axon_alert_e2e_")
    try:
        if use_cli:
            print(f"[info] mode: CLI --alert-webhook (empty config dir, no alert-dispatch.json)")
        else:
            channels: list[dict] = [
                {
                    "type": "webhook",
                    "id": "e2e_receiver",
                    "url": url,
                    "filters": {"severity": [], "alert_types": ["*"]},
                }
            ]
            if args.mcp_and_webhook:
                channels.insert(
                    0,
                    {
                        "type": "mcp",
                        "id": "mcp_client",
                        "filters": {"severity": [], "alert_types": ["*"]},
                    },
                )
            cfg = {"channels": channels}
            cfg_path = Path(cfg_dir) / "alert-dispatch.json"
            cfg_path.write_text(json.dumps(cfg), encoding="utf-8")
            print(f"[info] config: {cfg_path}")
        print(f"[info] webhook URL: {url}")

        env = os.environ.copy()
        if not use_cli:
            env["AXON_CONFIG_DIR"] = cfg_dir

        # Keep stdin open so MCP transport waits; collector still runs.
        stderr_target: int | None = subprocess.DEVNULL
        if os.environ.get("AXON_E2E_DEBUG"):
            stderr_target = None  # inherit (show tracing)
        if use_cli:
            cmd = [
                str(axon),
                "serve",
                "--config-dir",
                cfg_dir,
                "--alert-webhook",
                f"e2e_receiver={url}",
            ]
        else:
            cmd = [str(axon), "serve"]
        proc = subprocess.Popen(
            cmd,
            stdin=subprocess.PIPE,
            stdout=subprocess.DEVNULL,
            stderr=stderr_target,
            env=env,
        )
        assert proc.stdin is not None

        # CPU stress (macOS has `yes`)
        stressers: list = []
        ncpu = os.cpu_count() or 4
        for _ in range(ncpu):
            p = subprocess.Popen(
                ["yes"],
                stdout=subprocess.DEVNULL,
                stderr=subprocess.DEVNULL,
            )
            stressers.append(p)

        before_db = sqlite_alert_count()
        if before_db is not None:
            print(f"[info] alerts in SQLite before run: {before_db}")

        wait_s = int(os.environ.get("ALERT_E2E_WAIT", "90"))
        print(
            f"[info] axon serve PID {proc.pid}, stress {ncpu}x yes, "
            f"waiting up to {wait_s}s for state transitions (edge-triggered alerts)..."
        )
        for i in range(wait_s):
            time.sleep(1)
            with lock:
                n = len(posts)
            if n > 0:
                print(f"[ok] received {n} webhook POST(s) after {i + 1}s")
                break
            if i % 15 == 14:
                with lock:
                    n2 = len(posts)
                print(f"[info] ... {i + 1}s, webhook POSTs: {n2}")
        else:
            print(
                f"[warn] no webhook POST in {wait_s}s "
                "(steady state may produce zero transitions).",
                file=sys.stderr,
            )

        for p in stressers:
            p.terminate()
            try:
                p.wait(timeout=2)
            except subprocess.TimeoutExpired:
                p.kill()

        # Dispatcher uses fire-and-forget HTTP; give reqwest time before killing the process.
        time.sleep(5)

        proc.terminate()
        try:
            proc.wait(timeout=5)
        except subprocess.TimeoutExpired:
            proc.kill()

        srv.shutdown()

        with lock:
            bodies = list(posts)

        after_db = sqlite_alert_count()
        if after_db is not None:
            print(f"[info] alerts in SQLite after run: {after_db}")
            if before_db is not None and after_db > before_db:
                print(
                    f"[info] SQLite gained {after_db - before_db} row(s) "
                    "(unreliable if another axon/Cursor session is writing the same DB)"
                )

        if not bodies:
            print(
                "[err] no webhook POST received — triggers may not have fired, or HTTP was cancelled early.",
                file=sys.stderr,
            )
            print(
                "[info] Close other axon sessions, run with CPU/RAM load, or increase ALERT_E2E_WAIT.",
                file=sys.stderr,
            )
            return 1

        for i, body in enumerate(bodies[:5]):
            try:
                o = json.loads(body)
            except json.JSONDecodeError as e:
                print(f"[err] invalid JSON in POST {i}: {e}", file=sys.stderr)
                return 1
            for key in ("alert_type", "severity", "timestamp", "message", "metrics"):
                if key not in o:
                    print(f"[err] missing {key!r} in payload: {o!r}", file=sys.stderr)
                    return 1
            print(f"[ok] payload {i + 1}: type={o['alert_type']} severity={o['severity']}")

        print(f"[ok] consumed {len(bodies)} webhook(s); triggers + delivery working")
        return 0
    finally:
        shutil.rmtree(cfg_dir, ignore_errors=True)


if __name__ == "__main__":
    raise SystemExit(main())
