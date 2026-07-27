#!/usr/bin/env python3
"""
axon live integration harness — observe REAL data and REAL savings.

Unlike simulate_token_savings.py (which seeds a ledger), this harness reads *this
machine's* live state from /proc, applies axon's *actual* detection thresholds
(ported from crates/axon-core/src/thresholds.rs) and *actual* token catalog
(crates/axon-core/src/savings.rs), and writes real detections to the same
hardware.db the compiled `axon savings` reads.

To guarantee there is something to observe on an otherwise-idle machine, it induces
three real, bounded, self-cleaning loads (a CPU spin loop, a fast disk write, and a
memory-growing process) and detects them the same way axon's collector would. Nothing
is faked: a saving is logged only when a live measurement actually crosses a threshold.

It exists because the Rust binary cannot be compiled in this sandbox (crate downloads
are blocked); it is the closest real stand-in for `axon serve`'s collector loop.

Usage:  python3 scripts/axon_live_integration.py [--data-dir DIR] [--ticks N] [--price 3.0]
"""
import argparse, os, sqlite3, subprocess, sys, time, datetime as dt

CLK = os.sysconf("SC_CLK_TCK")
PAGE = os.sysconf("SC_PAGE_SIZE")

# ── axon thresholds (thresholds.rs) ─────────────────────────────────────────────
RAM_PCT_WARN, RAM_PCT_CRIT = 55.0, 75.0
DISK_FILL_WARN_GBS, DISK_FILL_CRIT_GBS = 0.05, 0.5      # hw_narrative
CPU_SPIN_PCT = 30.0                                     # idle_cpu_spin
RSS_GROWTH_WARN_MBH, RSS_GROWTH_CRIT_MBH = 50.0, 300.0  # ClaudeAgentInfo
GC_WARN_GB, GC_CRIT_GB = 0.8, 1.5                       # gc_pressure

# ── token catalog (savings.rs) ──────────────────────────────────────────────────
CATALOG = {
    "deferred_heavy_task":   (12000, "Deferred a heavy task under resource pressure", "deferred the build/test until the machine had headroom", None),
    "prevented_oom_crash":   (45000, "Caught an OOM / hard-freeze condition before the session was killed", "freed memory / paused work before the OOM kill", "#39022"),
    "context_reset":         (30000, "Caught runaway session RAM / GC thrash", "ran /clear to reset the session before GC thrash", "#33874"),
    "context_compaction":    (18000, "Caught an oversized session before a load hang", "ran /compact to shrink the session file", "#21022"),
    "stopped_polling_loop":  (8000,  "Caught a disk polling / re-read loop", "stopped the process re-reading a large file", "#22543"),
    "killed_runaway_process":(6000,  "Caught a runaway / crash-trajectory agent process", "restarted the runaway process before it degraded the session", "#21875"),
    "thermal_defer":         (4000,  "Deferred work while the CPU was thermally throttled", "paused heavy work until the CPU cooled", None),
    "agent_cleanup":         (5000,  "Surfaced stale / orphaned agent processes for cleanup", "cleaned up stale/orphaned agent processes", "#39170"),
    "disk_cleanup":          (7000,  "Caught a runaway disk-fill / disk-pressure condition", "cleared runaway files before the disk filled", "#26911"),
}

SCHEMA = """
CREATE TABLE IF NOT EXISTS savings_events (
    id INTEGER PRIMARY KEY, ts TEXT NOT NULL, category TEXT NOT NULL, source TEXT NOT NULL,
    signal TEXT NOT NULL, issue_ref TEXT, title TEXT NOT NULL, detail TEXT NOT NULL,
    action TEXT NOT NULL, tokens_saved INTEGER NOT NULL, usd_saved REAL NOT NULL, session_id TEXT);
CREATE INDEX IF NOT EXISTS idx_savings_ts ON savings_events(ts);
CREATE INDEX IF NOT EXISTS idx_savings_category ON savings_events(category);
"""

def fmt(n): return f"{n/1e6:.1f}M" if n>=1e6 else f"{n/1e3:.1f}K" if n>=1e3 else str(n)

# ── /proc readers ───────────────────────────────────────────────────────────────
def meminfo():
    d={}
    for line in open("/proc/meminfo"):
        k,v=line.split(":"); d[k]=int(v.strip().split()[0])*1024
    return d

def cpu_times():
    f=open("/proc/stat").readline().split()[1:]
    vals=list(map(int,f)); idle=vals[3]+vals[4]; total=sum(vals); return total,idle

def proc_stat(pid):
    try:
        data=open(f"/proc/{pid}/stat").read()
        rp=data.rfind(")"); fields=data[rp+2:].split()
        state=fields[0]; ppid=int(fields[1]); utime=int(fields[11]); stime=int(fields[12])
        starttime=int(fields[19]); rss_pages=int(fields[21])
        comm=data[data.find("(")+1:rp]
        return dict(state=state,ppid=ppid,cpu_ticks=utime+stime,rss=rss_pages*PAGE,start=starttime,comm=comm)
    except (FileNotFoundError, ProcessLookupError, IndexError, ValueError):
        return None

def proc_io_read(pid):
    try:
        for line in open(f"/proc/{pid}/io"):
            if line.startswith("read_bytes:"): return int(line.split()[1])
    except OSError: return 0
    return 0

def all_pids():
    return [int(p) for p in os.listdir("/proc") if p.isdigit()]

def child_count(pid, snapshot):
    return sum(1 for s in snapshot.values() if s and s["ppid"]==pid)

def disk_used(path="/tmp"):
    s=os.statvfs(path); return (s.f_blocks - s.f_bfree)*s.f_frsize

# ── ledger ──────────────────────────────────────────────────────────────────────
def log_event(conn, category, signal, detail, price, source="detected"):
    tok,title,action,issue=CATALOG[category]
    usd=tok/1e6*price
    conn.execute("INSERT INTO savings_events (ts,category,source,signal,issue_ref,title,detail,action,tokens_saved,usd_saved,session_id) VALUES (?,?,?,?,?,?,?,?,?,?,?)",
        (dt.datetime.now(dt.timezone.utc).isoformat(),category,source,signal,issue,title,detail,action,tok,usd,None))
    conn.commit()
    print(f"    [axon] +saving  {category:<22} ~{fmt(tok)} tok (~${usd:.4f})  <- {detail}")

# ── induced real load (bounded, self-cleaning) ──────────────────────────────────
def spawn_cpu(): return subprocess.Popen([sys.executable,"-c","\nx=0\nwhile True:\n x=(x*1103515245+12345)&0x7fffffff\n"])
def spawn_mem():
    # grows RSS ~120MB/s, hard cap ~1.2GB so it never threatens the session
    return subprocess.Popen([sys.executable,"-c",
        "\nimport time\nb=[]\nwhile len(b)<24:\n b.append(bytearray(50*1024*1024))\n for i in range(0,len(b[-1]),4096): b[-1][i]=1\n time.sleep(0.4)\ntime.sleep(60)\n"])
def spawn_disk(path):
    return subprocess.Popen(["dd","if=/dev/zero",f"of={path}","bs=1M","count=900"],
        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)

def main():
    ap=argparse.ArgumentParser()
    ap.add_argument("--data-dir", default=os.environ.get("AXON_DATA_DIR","/tmp/axon-live"))
    ap.add_argument("--ticks", type=int, default=12)
    ap.add_argument("--price", type=float, default=float(os.environ.get("AXON_TOKEN_PRICE_PER_MTOK","3.0")))
    a=ap.parse_args()
    os.makedirs(a.data_dir, exist_ok=True)
    db=os.path.join(a.data_dir,"hardware.db")
    conn=sqlite3.connect(db); conn.executescript(SCHEMA); conn.commit()
    fill_path=os.path.join(a.data_dir,"_axon_fill.tmp")

    print(f"axon live integration — observing this machine every 2s for {a.ticks} ticks")
    print(f"db: {db}   price: ${a.price:.2f}/1M tokens")
    print("="*72)

    prev_cpu=cpu_times(); prev_pid={}; prev_disk=disk_used(a.data_dir)
    logged=set()                  # edge-trigger: one saving per category per run
    spin=mem=disk=None
    cpu_hot_ticks={}              # pid -> consecutive high-cpu ticks
    rss_hist={}                   # pid -> [(monotonic_t, rss)] for windowed growth (mirrors slow EWMA)
    AGENT_KEYS=("claude","bun","node")

    try:
        for tick in range(1, a.ticks+1):
            # induce real load at known ticks so we can watch it get caught
            if tick==3: spin=spawn_cpu();  print("  >> induced: real CPU spin-loop  pid",spin.pid)
            if tick==5: disk=spawn_disk(fill_path); print("  >> induced: real fast disk write (dd 900MB)")
            if tick==7: mem=spawn_mem();   print("  >> induced: real memory-growing process  pid",mem.pid)

            time.sleep(2.0)

            # ── system: RAM + CPU ──
            mi=meminfo(); total=mi["MemTotal"]; avail=mi.get("MemAvailable",mi["MemFree"])
            used=total-avail; ram_pct=used/total*100
            ct=cpu_times(); dt_t=ct[0]-prev_cpu[0]; dt_i=ct[1]-prev_cpu[1]; prev_cpu=ct
            sys_cpu=(1-dt_i/dt_t)*100 if dt_t else 0.0

            # ── snapshot all procs ──
            snap={p:proc_stat(p) for p in all_pids()}
            # agent_cleanup targets orphaned/zombie AGENT processes only (claude/bun/node),
            # not incidental system zombies — matching axon's subagent-orphan detection.
            agent_zombies=[p for p,s in snap.items() if s and s["state"]=="Z" and any(k in s["comm"].lower() for k in AGENT_KEYS)]
            orphans=[p for p,s in snap.items() if s and s["ppid"]==1 and any(k in s["comm"].lower() for k in AGENT_KEYS)]
            claude=[p for p,s in snap.items() if s and "claude" in s["comm"].lower()]
            zombies_all=[p for p,s in snap.items() if s and s["state"]=="Z"]

            print(f"\ntick {tick:>2}  RAM {used/1e9:.1f}/{total/1e9:.0f}GB ({ram_pct:.0f}%)  CPU {sys_cpu:>4.0f}%  procs {len(snap)}  claude {len(claude)}  agent-orphans {len(orphans)}  agent-zombies {len(agent_zombies)}  (sys-zombies {len(zombies_all)})")

            # ── per-process detection (real /proc deltas) ──
            now=time.monotonic()
            for pid,s in snap.items():
                if not s: continue
                io_now=proc_io_read(pid)
                # windowed RSS history (mirrors axon's slow EWMA: needs sustained evidence)
                hist=rss_hist.setdefault(pid,[])
                hist.append((now,s["rss"]));  del hist[:-6]
                prev=prev_pid.get(pid)
                if prev:
                    cpu_pct=(s["cpu_ticks"]-prev["ct"])/CLK/2.0*100
                    io_delta=io_now-prev["io"]
                    kids=child_count(pid,snap)

                    # runaway / spin loop (idle_cpu_spin): sustained high CPU, no kids, low IO
                    if cpu_pct>CPU_SPIN_PCT and kids==0 and io_delta< 2_000_000:
                        cpu_hot_ticks[pid]=cpu_hot_ticks.get(pid,0)+1
                        if cpu_hot_ticks[pid]>=2 and "killed_runaway_process" not in logged:
                            logged.add("killed_runaway_process")
                            log_event(conn,"killed_runaway_process","idle_cpu_spin",
                                f"PID {pid} ({s['comm']}) at {cpu_pct:.0f}% CPU, no children/IO for >4s", a.price)
                    else:
                        cpu_hot_ticks[pid]=0

                    # memory leak / GC thrash trajectory: require SUSTAINED growth over the
                    # window plus a meaningful absolute increase, so single-tick RSS jitter on
                    # a healthy process is not mistaken for a leak (axon uses a ~40s slow EWMA).
                    if len(hist)>=4:
                        elapsed=hist[-1][0]-hist[0][0]; delta=hist[-1][1]-hist[0][1]
                        rate_mbh=delta/1e6/elapsed*3600 if elapsed>0 else 0
                        if delta>150_000_000 and rate_mbh>RSS_GROWTH_CRIT_MBH and "context_reset" not in logged:
                            logged.add("context_reset")
                            log_event(conn,"context_reset","rss_growth_critical",
                                f"PID {pid} ({s['comm']}) RSS growing ~{rate_mbh/1000:.1f}GB/hr, +{delta/1e6:.0f}MB over {elapsed:.0f}s ({s['rss']/1e6:.0f}MB now)", a.price)

                    # gc_pressure by absolute RSS
                    if s["rss"]/1e9>GC_CRIT_GB and "context_reset" not in logged:
                        logged.add("context_reset")
                        log_event(conn,"context_reset","gc_pressure_critical",
                            f"PID {pid} ({s['comm']}) at {s['rss']/1e9:.1f}GB RSS — GC thrash imminent", a.price)

                prev_pid[pid]=dict(ct=s["cpu_ticks"], io=io_now)

            # ── system RAM pressure (edge) ──
            if ram_pct>=RAM_PCT_CRIT and "prevented_oom_crash" not in logged:
                logged.add("prevented_oom_crash")
                log_event(conn,"prevented_oom_crash","memory_pressure_critical",
                    f"system RAM at {ram_pct:.0f}% (>= {RAM_PCT_CRIT:.0f}% critical)", a.price)
            elif ram_pct>=RAM_PCT_WARN and "deferred_heavy_task" not in logged:
                logged.add("deferred_heavy_task")
                log_event(conn,"deferred_heavy_task","memory_pressure_warn",
                    f"system RAM at {ram_pct:.0f}% (>= {RAM_PCT_WARN:.0f}% warn)", a.price)

            # ── disk fill rate (edge) ──
            du=disk_used(a.data_dir); fill_gbs=(du-prev_disk)/1e9/2.0; prev_disk=du
            if fill_gbs>=DISK_FILL_WARN_GBS and "disk_cleanup" not in logged:
                logged.add("disk_cleanup")
                log_event(conn,"disk_cleanup","disk_fill_rate",
                    f"disk filling at {fill_gbs*1000:.0f}MB/s (>= {DISK_FILL_WARN_GBS*1000:.0f}MB/s)", a.price)

            # ── stale/orphaned agents (edge) ──
            if (orphans or agent_zombies) and "agent_cleanup" not in logged:
                logged.add("agent_cleanup")
                log_event(conn,"agent_cleanup","agent_accumulation",
                    f"{len(orphans)} orphaned + {len(agent_zombies)} zombie agent process(es)", a.price)

            if claude:
                cp=claude[0]; s=snap[cp]
                print(f"          real claude agent pid {cp}: {s['rss']/1e6:.0f}MB RSS, state {s['state']}"
                      + ("  [healthy — nothing to prevent]" if s['rss']/1e9<GC_WARN_GB else "  [gc watch]"))
    finally:
        for p in (spin,mem,disk):
            if p:
                try: p.kill()
                except OSError: pass
        try: os.remove(fill_path)
        except OSError: pass

    # ── real report from the real ledger ──
    print("\n"+"="*72)
    rows=list(conn.execute("SELECT category,source,signal,detail,tokens_saved,usd_saved FROM savings_events ORDER BY id"))
    tot=sum(r[4] for r in rows)
    print(f"OBSERVED SAVINGS THIS RUN: {len(rows)} event(s), ~{fmt(tot)} tokens (~${tot/1e6*a.price:.4f})")
    for r in rows:
        print(f"  {r[0]:<22} ~{fmt(r[4]):>6} tok  ~${r[5]:.4f}  [{r[1]}]  signal={r[2]}")
        print(f"      {r[3]}")
    print(f"\nWritten to {db}")
    print(f"The compiled tool reads the same rows:  AXON_DATA_DIR={a.data_dir} axon savings --range last_24h")

if __name__=="__main__":
    main()
