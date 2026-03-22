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
