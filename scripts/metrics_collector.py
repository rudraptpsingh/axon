#!/usr/bin/env python3
"""
System Metrics Collector

Samples system metrics every 2 seconds (matching Axon's collector cadence).
Reads from /proc/stat, /proc/meminfo, os.statvfs, /sys/class/thermal.
Output: JSON array of timestamped samples.
"""
from __future__ import annotations

import json
import os
import signal
import sys
import time
from datetime import datetime, timezone
from pathlib import Path
from typing import Any


class MetricsCollector:
    """Collect system metrics from /proc and /sys."""

    def __init__(self, output_file: str, interval: float = 2.0):
        self.output_file = output_file
        self.interval = interval
        self.samples: list[dict[str, Any]] = []
        self.running = False
        self._prev_busy: int = 0
        self._prev_total: int = 0
        self._first_sample = True

    # ------------------------------------------------------------------
    # CPU
    # ------------------------------------------------------------------

    def _read_cpu_pct(self) -> float:
        """CPU usage from /proc/stat.  busy = user + nice + system (iowait excluded)."""
        try:
            with open("/proc/stat") as f:
                parts = f.readline().split()
            if len(parts) < 5:
                return 0.0

            user, nice, system, idle = (int(parts[i]) for i in range(1, 5))
            iowait = int(parts[5]) if len(parts) > 5 else 0

            busy = user + nice + system
            total = busy + idle + iowait

            if self._first_sample:
                self._prev_busy, self._prev_total = busy, total
                self._first_sample = False
                return 0.0

            d_busy = busy - self._prev_busy
            d_total = total - self._prev_total
            self._prev_busy, self._prev_total = busy, total

            if d_total <= 0:
                return 0.0
            return min(100.0, max(0.0, d_busy / d_total * 100.0))
        except Exception:
            return 0.0

    # ------------------------------------------------------------------
    # RAM
    # ------------------------------------------------------------------

    @staticmethod
    def _read_ram_pct() -> float:
        """RAM usage from /proc/meminfo."""
        try:
            info: dict[str, int] = {}
            with open("/proc/meminfo") as f:
                for line in f:
                    key, val = line.split(":", 1)
                    info[key.strip()] = int(val.split()[0])
            total = info.get("MemTotal", 1)
            avail = info.get("MemAvailable", 0)
            return min(100.0, max(0.0, (total - avail) / total * 100.0))
        except Exception:
            return 0.0

    # ------------------------------------------------------------------
    # Disk
    # ------------------------------------------------------------------

    @staticmethod
    def _read_disk_pct(mount: str = "/") -> float:
        """Disk usage via os.statvfs."""
        try:
            st = os.statvfs(mount)
            total = st.f_blocks * st.f_frsize
            free = st.f_bfree * st.f_frsize
            if total == 0:
                return 0.0
            return min(100.0, max(0.0, (total - free) / total * 100.0))
        except Exception:
            return 0.0

    # ------------------------------------------------------------------
    # Temperature
    # ------------------------------------------------------------------

    @staticmethod
    def _read_temp_c() -> float | None:
        """Die temperature from /sys/class/thermal or hwmon (millidegrees → C)."""
        # thermal_zone0 is the most common
        for zone in sorted(Path("/sys/class/thermal").glob("thermal_zone*")):
            try:
                raw = (zone / "temp").read_text().strip()
                return int(raw) / 1000.0
            except Exception:
                continue

        # Fallback: hwmon sensors
        hwmon = Path("/sys/class/hwmon")
        if hwmon.is_dir():
            for dev in sorted(hwmon.iterdir()):
                for tf in sorted(dev.glob("temp*_input")):
                    try:
                        return int(tf.read_text().strip()) / 1000.0
                    except Exception:
                        continue
        return None

    # ------------------------------------------------------------------
    # Public API
    # ------------------------------------------------------------------

    def sample(self) -> dict[str, Any]:
        """Collect one sample of all metrics."""
        ts = datetime.now(timezone.utc).isoformat()
        s: dict[str, Any] = {
            "timestamp": ts,
            "cpu_pct": round(self._read_cpu_pct(), 2),
            "ram_pct": round(self._read_ram_pct(), 2),
            "disk_pct": round(self._read_disk_pct(), 2),
        }
        temp = self._read_temp_c()
        if temp is not None:
            s["temp_c"] = round(temp, 1)
        return s

    def run(self, duration_s: float | None = None) -> None:
        """Collect samples until SIGTERM/SIGINT or optional duration elapses."""
        self.running = True

        def _stop(sig: int, _: Any) -> None:
            self.running = False

        signal.signal(signal.SIGINT, _stop)
        signal.signal(signal.SIGTERM, _stop)

        deadline = time.time() + duration_s if duration_s else None
        print(f"[info] metrics_collector: sampling every {self.interval}s → {self.output_file}", file=sys.stderr)

        while self.running:
            if deadline and time.time() >= deadline:
                break
            s = self.sample()
            self.samples.append(s)
            time.sleep(self.interval)

        self.save()

    def save(self) -> None:
        """Persist samples to JSON."""
        os.makedirs(os.path.dirname(self.output_file) or ".", exist_ok=True)
        with open(self.output_file, "w") as f:
            json.dump(self.samples, f, indent=2)
        print(f"[ok] metrics_collector: {len(self.samples)} samples → {self.output_file}", file=sys.stderr)


def main() -> int:
    import argparse

    ap = argparse.ArgumentParser(description="Collect system metrics every N seconds")
    ap.add_argument("output_file", help="JSON file to write metrics to")
    ap.add_argument("--interval", type=float, default=2.0)
    ap.add_argument("--duration", type=float, default=None, help="Stop after N seconds")
    args = ap.parse_args()

    MetricsCollector(args.output_file, interval=args.interval).run(duration_s=args.duration)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
