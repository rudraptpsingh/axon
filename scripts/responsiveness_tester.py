#!/usr/bin/env python3
"""
System Responsiveness Tester

Measures system responsiveness by timing basic commands (ls, ps, uptime).
Detects degradation during stress by tracking wall-clock latency.
Output: JSON array of timestamped command latencies.
"""
from __future__ import annotations

import json
import os
import signal
import subprocess
import sys
import time
from datetime import datetime, timezone
from typing import Any


COMMANDS: list[tuple[list[str], str, float]] = [
    # (cmd, label, timeout_s)
    (["ls", "/"], "ls_root", 10.0),
    (["ps", "aux"], "ps_aux", 15.0),
    (["uptime"], "uptime", 10.0),
]


class ResponsivenessTester:
    """Measure system responsiveness via command execution latency."""

    def __init__(self, output_file: str, interval: float = 5.0):
        self.output_file = output_file
        self.interval = interval
        self.samples: list[dict[str, Any]] = []
        self.running = False

    @staticmethod
    def _time_command(cmd: list[str], label: str, timeout: float) -> dict[str, Any]:
        ts = datetime.now(timezone.utc).isoformat()
        t0 = time.monotonic()
        try:
            r = subprocess.run(cmd, capture_output=True, text=True, timeout=timeout)
            elapsed_ms = (time.monotonic() - t0) * 1000.0
            return {
                "timestamp": ts,
                "command": label,
                "latency_ms": round(elapsed_ms, 2),
                "exit_code": r.returncode,
            }
        except subprocess.TimeoutExpired:
            elapsed_ms = (time.monotonic() - t0) * 1000.0
            return {
                "timestamp": ts,
                "command": label,
                "latency_ms": round(elapsed_ms, 2),
                "exit_code": -1,
                "error": "timeout",
            }
        except Exception as e:
            elapsed_ms = (time.monotonic() - t0) * 1000.0
            return {
                "timestamp": ts,
                "command": label,
                "latency_ms": round(elapsed_ms, 2),
                "exit_code": -1,
                "error": str(e),
            }

    def run(self, duration_s: float | None = None) -> None:
        self.running = True

        def _stop(sig: int, _: Any) -> None:
            self.running = False

        signal.signal(signal.SIGINT, _stop)
        signal.signal(signal.SIGTERM, _stop)

        deadline = time.time() + duration_s if duration_s else None
        print(f"[info] responsiveness: interval={self.interval}s → {self.output_file}", file=sys.stderr)

        while self.running:
            if deadline and time.time() >= deadline:
                break
            for cmd, label, timeout in COMMANDS:
                s = self._time_command(cmd, label, timeout)
                self.samples.append(s)
            time.sleep(self.interval)

        self.save()

    def save(self) -> None:
        os.makedirs(os.path.dirname(self.output_file) or ".", exist_ok=True)
        with open(self.output_file, "w") as f:
            json.dump(self.samples, f, indent=2)
        print(f"[ok] responsiveness: {len(self.samples)} samples → {self.output_file}", file=sys.stderr)


def main() -> int:
    import argparse

    ap = argparse.ArgumentParser(description="Measure system responsiveness")
    ap.add_argument("output_file")
    ap.add_argument("--interval", type=float, default=5.0)
    ap.add_argument("--duration", type=float, default=None)
    args = ap.parse_args()

    ResponsivenessTester(args.output_file, interval=args.interval).run(duration_s=args.duration)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
