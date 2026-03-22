#!/usr/bin/env python3
"""
Webhook Receiver for Axon Alerts

Instance-based, thread-safe HTTP server that captures Axon alert webhook POSTs.
Binds to 127.0.0.1 on a random free port (matching perf_test_scenario.py pattern).
Path: /alerts (matching Axon convention and alert_receiver_minimal.py).
"""
from __future__ import annotations

import json
import signal
import sys
import threading
import time
from http.server import BaseHTTPRequestHandler, HTTPServer
from typing import Any


class WebhookCollector:
    """Thread-safe webhook receiver for Axon alerts.

    Modelled after perf_test_scenario.py WebhookCollector.
    """

    def __init__(self) -> None:
        self.posts: list[dict[str, Any]] = []
        self.timestamps: list[float] = []
        self.lock = threading.Lock()
        self.server: HTTPServer | None = None
        self.port = 0
        self.url = ""

    def start(self) -> None:
        """Start HTTP server on a random free port in a daemon thread."""
        collector = self

        class H(BaseHTTPRequestHandler):
            def do_POST(self) -> None:
                ln = int(self.headers.get("Content-Length", 0))
                raw = self.rfile.read(ln)
                try:
                    obj = json.loads(raw)
                    with collector.lock:
                        collector.posts.append(obj)
                        collector.timestamps.append(time.time())
                    print(
                        f"[info] webhook: alert received — "
                        f"{obj.get('alert_type', '?')} ({obj.get('severity', '?')})",
                        file=sys.stderr,
                    )
                except json.JSONDecodeError:
                    pass
                self.send_response(200)
                self.end_headers()
                self.wfile.write(b"ok")

            def log_message(self, *_: object) -> None:
                pass

        self.server = HTTPServer(("127.0.0.1", 0), H)
        self.port = self.server.server_address[1]
        self.url = f"http://127.0.0.1:{self.port}/alerts"
        t = threading.Thread(target=self.server.serve_forever, daemon=True)
        t.start()
        print(f"[ok] webhook: listening at {self.url}", file=sys.stderr)

    def stop(self) -> None:
        if self.server:
            self.server.shutdown()

    def wait_for_alert(self, timeout_s: float = 60.0) -> float | None:
        """Wait for the first alert.  Returns epoch timestamp or None on timeout."""
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            with self.lock:
                if self.timestamps:
                    return self.timestamps[0]
            time.sleep(0.5)
        return None

    def get_alerts(self) -> list[dict[str, Any]]:
        with self.lock:
            return list(self.posts)

    def get_timestamps(self) -> list[float]:
        with self.lock:
            return list(self.timestamps)

    def save(self, output_file: str) -> None:
        """Persist received alerts to JSON."""
        import os

        os.makedirs(os.path.dirname(output_file) or ".", exist_ok=True)
        with self.lock:
            data = list(self.posts)
        with open(output_file, "w") as f:
            json.dump(data, f, indent=2)
        print(f"[ok] webhook: {len(data)} alerts → {output_file}", file=sys.stderr)


def main() -> int:
    """Run standalone webhook receiver (for manual testing)."""
    import argparse

    ap = argparse.ArgumentParser(description="Receive Axon alert webhooks")
    ap.add_argument("--output", help="JSON file to write alerts to")
    ap.add_argument("--duration", type=float, help="Stop after N seconds")
    args = ap.parse_args()

    wh = WebhookCollector()
    wh.start()

    def _stop(sig: int, _: Any) -> None:
        wh.stop()
        if args.output:
            wh.save(args.output)
        raise SystemExit(0)

    signal.signal(signal.SIGINT, _stop)
    signal.signal(signal.SIGTERM, _stop)

    if args.duration:
        time.sleep(args.duration)
        wh.stop()
        if args.output:
            wh.save(args.output)
    else:
        print("[info] webhook: Ctrl+C to stop", file=sys.stderr)
        while True:
            time.sleep(1)

    return 0


if __name__ == "__main__":
    raise SystemExit(main())
