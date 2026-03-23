You are evaluating axon — a zero-cloud MCP server that gives AI coding agents real-time hardware awareness. You ARE the agent. axon is YOUR tool.

## Setup
1. Build and install axon: `cargo install --path crates/axon-cli`
2. Run `axon setup claude-code`
3. Verify: `axon diagnose` and `axon query hw_snapshot`

## Evaluation Loop (repeat 5 times minimum)

For each iteration:

### Step 1: Do real work while consulting axon
- Before heavy tasks (cargo build, cargo test, file writes): run `axon query hw_snapshot` — check headroom field. If insufficient, reduce parallelism or defer.
- During stress: run `axon query process_blame` — check if culprit/group/fix are accurate and actionable.
- After stress: run `axon diagnose` — check if recovery is detected, if recent alerts show.
- End of session: run `axon query session_health` — check if the narrative captures what happened.

### Step 2: Generate stress to test detection
- CPU stress: `for i in 1 2 3 4; do yes > /dev/null & done` then observe axon
- Memory stress: `python3 -c "x=[bytearray(100_000_000) for _ in range(50)]"` then observe
- Mixed: run cargo clean && cargo build --release -j 4 with yes workers simultaneously
- Kill stress and immediately diagnose — check recovery indicator
- I/O stress: `dd if=/dev/zero of=/tmp/axon_testfile bs=1M count=4096 conv=fdatasync` then observe — check if impact score reflects disk saturation
- Gradual leak: `python3 -c "import time; x=[]; [x.append(bytearray(50_000_000)) or time.sleep(2) for _ in range(20)]"` — watch for slow drift detection over ~40s
- Flap test: rapidly alternate stress and idle (5s on, 5s off, repeat 6 times) — verify no alert storm, hysteresis prevents flapping
- Recovery test: start stress, wait 10s, kill stress, verify recovery/resolved alert fires within 10s
- Alert verification: after stress, run `axon query session_health` -- check alert_count > 0; if 0, alerts are not firing when they should
- Serve lifecycle: start `axon serve` via MCP stdio, run all 7 tools, verify serve process stays alive throughout, verify clean exit on stdin close
- GPU snapshot: run `axon query gpu_snapshot` -- verify ok=true and detected field is present

### Step 3: Evaluate every signal axon gives you
For each axon response, ask:
- Is the **culprit** correct? (right process, right group)
- Is the **anomaly_type** correct? (cpu_saturation vs agent_accumulation vs none)
- Is the **impact_level** proportional to actual stress?
- Is the **headroom** field actionable? Would it change my decision?
- Is the **narrative** coherent? (no contradictions, no redundancy)
- Is the **fix suggestion** specific and correct?
- Are **alerts** firing when they should? Not firing when they shouldn't?
- Does **session_health** accurately reflect what happened?
- Are **recovery alerts** firing when stress ends? (resolved notifications)
- Is **I/O saturation** reflected in impact scoring during builds or dd stress?
- Are **hysteresis bands** preventing flap alerts near thresholds?
- Does the **slow EWMA** catch gradual memory leaks that fast EWMA misses?
- Does **gpu_snapshot** return valid JSON? (ok=true, detected field present)
- Does **hardware_trend** show stress period in buckets? (CPU spike visible)
- Is the **serve process** still alive after all queries? (no crash mid-session)
- Do all **7 MCP tools** respond to tools/list? (hw_snapshot, process_blame, battery_status, system_profile, hardware_trend, session_health, gpu_snapshot)
- Are **alerts firing** during stress? (session_health.alert_count > 0 after CPU saturation)
- Does the **ring buffer** provide faster trend queries than SQLite?

### Step 4: Fix what's broken
- Read the relevant source file (types.rs, impact.rs, collector.rs, alerts.rs, thresholds.rs, lib.rs, main.rs)
- Make the minimal fix
- Add a unit test if the fix changes thresholds/logic
- Run `cargo test --workspace` — all must pass
- Rebuild: `cargo build --release`
- Re-run the same scenario that exposed the bug — verify the fix

### Step 5: Record findings
After each fix, note:
- What signal was wrong
- What the root cause was
- What you changed
- Before vs after comparison

## What to look for (priority order)
1. Wrong culprit attribution (blaming the wrong process)
2. Impact level not matching reality (healthy when system is pegged)
3. Headroom not transitioning (same level at 0% and 100% CPU)
4. Alerts not firing (or firing spuriously)
5. Contradictory narratives (impact says X, fix says Y)
6. Missing context in narratives (no reason string, no CPU/RAM details)
7. EWMA warmup blindness (blame_score=0 for everything in short queries)
8. Recovery amnesia (no memory of recent stress)
9. Agent accumulation masking real culprits
10. Score formula underweighting single-resource saturation
11. Alert flapping (same alert firing/resolving repeatedly in <30s)
12. Missing recovery signals (system recovers but no resolved notification)
13. Slow drift blindness (gradual memory leak over 5+ minutes not detected)
14. I/O-blind impact scoring (cargo build saturating disk but impact=Healthy)
15. Threshold rigidity (55% RAM warn on a 128GB machine is too aggressive)
16. Spike false alarms (momentary 1-tick spike triggers full alert chain)
17. Missing CLI query tools (gpu_snapshot/hardware_trend not dispatched by `axon query`)
18. Cumulative iowait (lifetime average instead of instantaneous delta between ticks)
19. Serve lifecycle issues (process dies mid-session, no clean exit on stdin close)

## Constraints
- Do NOT increase `axon diagnose` duration — it must stay at 4 seconds
- Do NOT add network calls or telemetry
- Do NOT break existing tests — always run `cargo test --workspace` after changes
- Keep fixes minimal — change the least code necessary
- Commit after each round with clear before/after in the commit message

## End of session
- Run the full eval script: `python3 scripts/eval_cpu_stress.py ./target/release/axon`
- ALL CHECKS must PASS
- Report: total fixes made, decision attribution (% of your decisions informed by axon), remaining gaps
- Commit and push to your branch
