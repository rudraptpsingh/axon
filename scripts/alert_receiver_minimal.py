#!/usr/bin/env python3
"""
Minimal local server to consume Axon webhook alerts (JSON POST bodies).

Run:
  python3 scripts/alert_receiver_minimal.py

Copy the printed URL into ~/.config/axon/alert-dispatch.json under a webhook
channel, then restart the process that runs `axon serve` (e.g. reload MCP in Cursor).

Example alert-dispatch.json fragment:
  {
    "channels": [
      {
        "type": "webhook",
        "id": "local",
        "url": "<PASTE URL FROM THIS SCRIPT>",
        "filters": { "severity": [], "alert_types": ["*"] }
      }
    ]
  }

Automated proof that POST delivery works: cargo test -p axon-core --test alert_integration
"""
from __future__ import annotations

import argparse
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer


def main() -> int:
    parser = argparse.ArgumentParser(description="Minimal axon webhook receiver")
    parser.add_argument("--output-file", metavar="PATH", help="Write each alert JSON to this file (overwrites)")
    args = parser.parse_args()

    output_file: str | None = args.output_file

    class H(BaseHTTPRequestHandler):
        def do_POST(self) -> None:
            ln = int(self.headers.get("Content-Length", 0))
            raw = self.rfile.read(ln)
            try:
                obj = json.loads(raw)
                formatted = json.dumps(obj, indent=2)
                print(formatted, flush=True)
                if output_file:
                    with open(output_file, "w") as f:
                        f.write(formatted + "\n")
            except json.JSONDecodeError:
                print(raw.decode(errors="replace"), flush=True)
            self.send_response(200)
            self.end_headers()
            self.wfile.write(b"ok\n")

        def log_message(self, *args: object) -> None:
            pass

    srv = HTTPServer(("127.0.0.1", 0), H)
    port = srv.server_address[1]
    url = f"http://127.0.0.1:{port}/alerts"
    print(f"[ok] POST alerts to: {url}", flush=True)
    print("[info] Ctrl+C to stop", flush=True)
    try:
        srv.serve_forever()
    except KeyboardInterrupt:
        pass
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
