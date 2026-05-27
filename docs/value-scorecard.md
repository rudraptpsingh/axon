# Axon Value Scorecard

The value scorecard turns Axon runtime signals into user-facing business
metrics: avoided tool spawns, capped parallelism, estimated credits saved,
estimated time saved, and workflow impacts.

## Run

```bash
python3 scripts/axon_value_scorecard.py target/debug/axon --prove --out agent_value_scorecard.md
```

`--prove` temporarily starts local MCP-like helper processes to show what would
happen without an Axon policy, then cleans them up and verifies the runtime
returns to baseline.

## Example Output

```text
Risky tool spawns avoided: 4
Parallel workers avoided: 3
Estimated credits saved: 4.75
Estimated time saved: 4.5 min
No-policy MCP count: 39 -> 43
Cleanup MCP count: 39
```

## Why It Matters

Users do not buy a process list. They buy fewer failed runs, fewer wasted
credits, less waiting, and agents that know when to reduce or reuse instead of
blindly spawning more work.

The scorecard is designed to answer:

- What would the agent have done without Axon?
- What did Axon prevent?
- What safe work was still allowed?
- How much time or credit burn was avoided?
- What should the user or agent do next?

## Tunable Assumptions

```bash
python3 scripts/axon_value_scorecard.py target/debug/axon \
  --requested-tool-spawns 4 \
  --requested-parallelism 4 \
  --credit-per-tool-spawn 1.0 \
  --credit-per-parallel-worker 0.25 \
  --seconds-per-tool-spawn 45 \
  --seconds-per-parallel-worker 30 \
  --hourly-developer-cost 100 \
  --prove
```

These defaults are intentionally conservative placeholders. For an app-builder
platform, replace them with real credit pricing, average retry duration, and
support/developer cost assumptions.
