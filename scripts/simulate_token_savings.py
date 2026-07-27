#!/usr/bin/env python3
"""
Schema-faithful simulation of axon's token-savings ledger.

This script creates the *same* SQLite table (`savings_events`) that
`axon-core/src/persistence.rs` creates, inserts a realistic week of savings
events using the *same* conservative token catalog as
`axon-core/src/savings.rs`, and prints the *same* report as the CLI
`axon savings` command (`print_savings_report` in `axon-cli/src/main.rs`).

Because the schema and SQL are identical, the compiled `axon` binary reads the
very same rows: running `AXON_DATA_DIR=<dir> axon savings` after this script
prints the same numbers. It exists so the end-user output can be demonstrated
without the Rust toolchain (crate downloads are blocked in this sandbox).

Usage:
    python3 scripts/simulate_token_savings.py [--data-dir DIR] [--price 3.0] [--range last_7d]
"""
import argparse
import os
import sqlite3
import datetime as dt

# ── Catalog: mirrors SAVINGS_CATALOG in axon-core/src/savings.rs ──────────────
CATALOG = {
    "deferred_heavy_task": (12000, "Deferred a heavy task under resource pressure",
                            "deferred the build/test until the machine had headroom", None),
    "prevented_oom_crash": (45000, "Caught an OOM / hard-freeze condition before the session was killed",
                            "freed memory / paused work before the OOM kill", "#39022"),
    "context_reset": (30000, "Caught runaway session RAM / GC thrash",
                      "ran /clear to reset the session before GC thrash", "#33874"),
    "context_compaction": (18000, "Caught an oversized session before a load hang",
                           "ran /compact to shrink the session file", "#21022"),
    "stopped_polling_loop": (8000, "Caught a disk polling / re-read loop",
                             "stopped the process re-reading a large file", "#22543"),
    "killed_runaway_process": (6000, "Caught a runaway / crash-trajectory agent process",
                               "restarted the runaway process before it degraded the session", "#21875"),
    "thermal_defer": (4000, "Deferred work while the CPU was thermally throttled",
                      "paused heavy work until the CPU cooled", None),
    "agent_cleanup": (5000, "Surfaced stale / orphaned agent processes for cleanup",
                      "cleaned up stale/orphaned agent processes", "#39170"),
    "disk_cleanup": (7000, "Caught a runaway disk-fill / disk-pressure condition",
                     "cleared runaway files before the disk filled", "#26911"),
}

SCHEMA = """
CREATE TABLE IF NOT EXISTS savings_events (
    id INTEGER PRIMARY KEY,
    ts TEXT NOT NULL,
    category TEXT NOT NULL,
    source TEXT NOT NULL,
    signal TEXT NOT NULL,
    issue_ref TEXT,
    title TEXT NOT NULL,
    detail TEXT NOT NULL,
    action TEXT NOT NULL,
    tokens_saved INTEGER NOT NULL,
    usd_saved REAL NOT NULL,
    session_id TEXT
);
CREATE INDEX IF NOT EXISTS idx_savings_ts ON savings_events(ts);
CREATE INDEX IF NOT EXISTS idx_savings_category ON savings_events(category);
"""

# A realistic week of daily Claude-Code usage. (day_offset, hour, category, source, signal, detail)
# day_offset counts back from "today" (0 = today).
PLAN = [
    (6, 9,  "deferred_heavy_task",   "detected",     "memory_pressure_warn",     "RAM at 88% before a cargo build"),
    (6, 14, "context_compaction",    "agent_action", "cli",                      "session file ~28MB -> ran /compact"),
    (5, 10, "stopped_polling_loop",  "detected",     "io_read_polling",          "PID 4821 reading 190MB/s re-reading a binary"),
    (5, 16, "prevented_oom_crash",   "detected",     "memory_pressure_critical", "RAM 96% + swap exhausted before OOM kill"),
    (4, 11, "context_reset",         "agent_action", "cli",                      "claude at 2.1GB GC-thrashing -> ran /clear"),
    (4, 15, "agent_cleanup",         "detected",     "agent_accumulation",       "4 orphaned bun MCP servers from a crashed session"),
    (3, 9,  "thermal_defer",         "detected",     "thermal_throttle",         "CPU throttling at 99C; paused test suite"),
    (3, 13, "deferred_heavy_task",   "agent_action", "cli",                      "headroom=insufficient -> deferred docker build"),
    (2, 10, "disk_cleanup",          "detected",     "disk_pressure",            "/tmp/claude-* filling at 60MB/s"),
    (2, 17, "killed_runaway_process","detected",     "runaway_agent",            "PID 5120 spin-looping at 98% CPU"),
    (1, 8,  "context_compaction",    "detected",     "session_file_oversized",   "session file ~44MB load-hang risk"),
    (1, 12, "prevented_oom_crash",   "agent_action", "cli",                      "freed 3GB before OOM on 8GB laptop"),
    (1, 19, "context_reset",         "detected",     "gc_pressure_critical",     "PID 6001 at 1.9GB RAM, GC thrash imminent"),
    (0, 9,  "stopped_polling_loop",  "agent_action", "cli",                      "stopped re-reading node_modules dir every turn"),
    (0, 11, "deferred_heavy_task",   "detected",     "impact_escalation",        "impact escalated to critical before test run"),
    (0, 15, "agent_cleanup",         "agent_action", "cli",                      "killed 2 stale 30h claude sessions holding RAM"),
]


def tokens_to_usd(tokens, price):
    return tokens / 1_000_000.0 * price


def fmt_tokens(n):
    if n >= 1_000_000:
        return f"{n/1_000_000.0:.1f}M"
    if n >= 1_000:
        return f"{n/1_000.0:.1f}K"
    return str(n)


def seed(conn, price):
    conn.executescript(SCHEMA)
    now = dt.datetime.now(dt.timezone.utc)
    for day_off, hour, cat, source, signal, detail in PLAN:
        base_tokens, title, action, issue = CATALOG[cat]
        ts = (now - dt.timedelta(days=day_off)).replace(
            hour=hour, minute=(day_off * 7 + hour) % 60, second=0, microsecond=0)
        usd = tokens_to_usd(base_tokens, price)
        conn.execute(
            "INSERT INTO savings_events (ts, category, source, signal, issue_ref, title, detail, action, tokens_saved, usd_saved, session_id) "
            "VALUES (?,?,?,?,?,?,?,?,?,?,?)",
            (ts.isoformat(), cat, source, signal, issue, title, detail, action, base_tokens, usd, None),
        )
    conn.commit()


def parse_range(s):
    day = 86400
    return {
        "last_24h": (day, day), "last_7d": (7 * day, day),
        "last_30d": (30 * day, day), "last_90d": (90 * day, 7 * day),
    }.get(s)


def report(conn, range_label, range_secs, bucket_secs, price, recent_limit=15):
    since = dt.datetime.now(dt.timezone.utc) - dt.timedelta(seconds=range_secs)
    rows = list(conn.execute(
        "SELECT ts, category, source, signal, issue_ref, title, detail, action, tokens_saved, usd_saved "
        "FROM savings_events WHERE ts >= ? ORDER BY ts DESC", (since.isoformat(),)))

    total_events = len(rows)
    total_tokens = sum(r[8] for r in rows)
    detected = sum(1 for r in rows if r[2] != "agent_action")
    confirmed = sum(1 for r in rows if r[2] == "agent_action")

    # by category
    cat_map = {}
    for r in rows:
        c, t = r[1], r[8]
        ec, tk = cat_map.get(c, (0, 0))
        cat_map[c] = (ec + 1, tk + t)
    by_cat = sorted(cat_map.items(), key=lambda kv: -kv[1][1])

    # by bucket (align to epoch like the Rust code's div_euclid)
    buckets = {}
    for r in rows:
        ts = dt.datetime.fromisoformat(r[0])
        idx = int(ts.timestamp()) // bucket_secs
        ec, tk = buckets.get(idx, (0, 0))
        buckets[idx] = (ec + 1, tk + r[8])
    bucket_is_day = bucket_secs % 86400 == 0

    print()
    print(f"axon -- token & cost savings ({range_label})")
    print("=======================================================")
    if total_events == 0:
        print("\nNo savings recorded yet in this window.\n")
        return
    print()
    print(f"Total saved:   ~{fmt_tokens(total_tokens)} tokens   (~${tokens_to_usd(total_tokens, price):.2f})   across {total_events} event(s)")
    print(f"Attribution:   {detected} detected by axon, {confirmed} confirmed by an agent")
    print(f"Price basis:   ${price:.2f} / 1M tokens (conservative estimates, not measurements)")

    print("\nBy period:")
    for idx in sorted(buckets):
        ec, tk = buckets[idx]
        start = dt.datetime.fromtimestamp(idx * bucket_secs, dt.timezone.utc)
        label = start.strftime("%Y-%m-%d") if bucket_is_day else start.strftime("%Y-%m-%d %H:%M")
        print(f"  {label:<16}  {ec:>3} event(s)   ~{fmt_tokens(tk):>7} tokens   ~${tokens_to_usd(tk, price):.2f}")

    print("\nBy category:")
    for c, (ec, tk) in by_cat:
        print(f"  {c:<24}  {ec:>3}   ~{fmt_tokens(tk):>7} tokens   ~${tokens_to_usd(tk, price):.2f}")

    print("\nRecent events:")
    for r in rows[:recent_limit]:
        ts = dt.datetime.fromisoformat(r[0])
        issue = f" ({r[4]})" if r[4] else ""
        print(f"  [{ts.strftime('%m-%d %H:%M')}] {r[1]}{issue}   ~{fmt_tokens(r[8])} tok  ~${r[9]:.4f}  [{r[2]}]")
        print(f"      {r[5]}")
        print(f"      signal: {r[3]} | action: {r[7]}")
    print()


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--data-dir", default=os.environ.get("AXON_DATA_DIR", "/tmp/axon-sim"))
    ap.add_argument("--price", type=float, default=float(os.environ.get("AXON_TOKEN_PRICE_PER_MTOK", "3.0")))
    ap.add_argument("--range", default="last_7d")
    args = ap.parse_args()

    os.makedirs(args.data_dir, exist_ok=True)
    db_path = os.path.join(args.data_dir, "hardware.db")
    fresh = not os.path.exists(db_path)
    conn = sqlite3.connect(db_path)
    # Only seed once so re-running against a real axon DB does not duplicate rows.
    if fresh or conn.execute("SELECT COUNT(*) FROM sqlite_master WHERE name='savings_events'").fetchone()[0] == 0:
        seed(conn, args.price)
    elif conn.execute("SELECT COUNT(*) FROM savings_events").fetchone()[0] == 0:
        seed(conn, args.price)

    rng = parse_range(args.range)
    if not rng:
        raise SystemExit(f"bad range {args.range}")
    report(conn, args.range, rng[0], rng[1], args.price)
    print(f"[db] {db_path}  ->  compiled tool reads the same rows via:  AXON_DATA_DIR={args.data_dir} axon savings --range {args.range}")


if __name__ == "__main__":
    main()
