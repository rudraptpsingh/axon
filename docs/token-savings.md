# Token & cost savings

axon does not just observe hardware — it changes what your AI coding agent *does*, and
every actionable signal it surfaces lets the agent avoid a concrete, wasteful token spend:

- a build that would OOM and be re-run,
- a session that would crash and have its whole context rebuilt from scratch,
- a polling loop re-reading a large file into context every turn,
- a bloated session file re-processed on every request.

axon turns those prevented failures into a running estimate of **tokens (and dollars) saved**,
so a developer running ordinary Claude sessions can look back and see:

> axon saved me ~265K tokens (~$0.80) this week — here are the 16 moments behind that number.

Everything is local. The ledger lives in the same on-device SQLite database as the rest of
axon (`hardware.db`); nothing is ever sent off-device.

## The two easy ways to use it

### 1. As a Claude Code skill (no MCP config)

```
axon setup claude-code-skill
```

This installs `~/.claude/skills/axon/SKILL.md`. The skill drives axon entirely through the
`axon` CLI, so no MCP server configuration is required — dropping the skill in is enough for
Claude to start checking hardware before heavy work and logging the tokens it saves. A
reference copy lives at [`skills/axon/SKILL.md`](../skills/axon/SKILL.md).

### 2. As MCP tools

`axon setup` (or `axon setup claude-code`) also registers the MCP server, which exposes two
new tools:

- `token_savings` — total tokens/dollars saved, a per-category breakdown, a daily/weekly
  rollup for trends, and the most recent referenced events.
- `record_savings` — the agent calls this after acting on an axon recommendation, so the
  prevented spend is logged.

## Seeing your savings

```
axon savings                    # this week (default)
axon savings --range last_24h   # today
axon savings --range last_30d   # this month
axon savings --json             # machine-readable
```

Each event carries a reference: the signal that triggered it, the related upstream issue,
and the action taken — so every number is backed by concrete moments, not a black box.

## Recording a saving

The agent (or you) logs a confirmed saving:

```
axon savings record --category deferred_heavy_task \
    --detail "hw_snapshot showed headroom=insufficient (RAM 94%), so I deferred the cargo build"
```

axon also records preventions it detects on its own (memory/thermal/agent conditions),
edge-triggered so the ledger fills in without spamming.

## Categories and estimates

Token estimates are deliberately **conservative** and centralised in
[`crates/axon-core/src/savings.rs`](../crates/axon-core/src/savings.rs) (the `catalog`
function) so they are easy to audit and tune. They are estimates, not measurements, and
every surfaced number is labelled as such.

| Category                 | Est. tokens | Prevents |
| ------------------------ | ----------: | -------- |
| `prevented_oom_crash`    |      45,000 | full context rebuild after an OOM-killed session |
| `context_reset`          |      30,000 | re-sending a bloated context every turn (`/clear`) |
| `context_compaction`     |      18,000 | re-processing an oversized session (`/compact`) |
| `deferred_heavy_task`    |      12,000 | a failed build cycle: reading the error and retrying |
| `stopped_polling_loop`   |       8,000 | repeatedly pulling a large file into context |
| `disk_cleanup`           |       7,000 | a disk-full crash and restart cycle |
| `killed_runaway_process` |       6,000 | a degraded, low-productivity session |
| `agent_cleanup`          |       5,000 | redundant re-spawns across leaked sessions |
| `thermal_defer`          |       4,000 | slow token generation during a throttle window |

## Cost model

Dollars = tokens ÷ 1,000,000 × price-per-million-tokens.

The blended price defaults to a conservative **$3.00 / 1M tokens** and is configurable with
`AXON_TOKEN_PRICE_PER_MTOK`. Real usage on larger models costs several times more, so the
dollar figure scales with the model you actually run — the token figure is the honest unit.

## Try it without a full build

Two scripts write to the **same** `savings_events` table the compiled `axon savings` reads,
so you can see the pipeline end-to-end before building:

- `scripts/simulate_token_savings.py` seeds a realistic week using the same catalog and
  prints the same report — useful for a quick demo of the report format.
- `scripts/axon_live_integration.py` is a **live integration harness**: it reads *this
  machine's* real `/proc` state every 2s, applies axon's real thresholds
  (`thresholds.rs`) and token catalog (`savings.rs`), and logs **real** detections — only
  when a live measurement actually crosses a threshold. To guarantee there is something to
  observe on an idle machine it induces three real, bounded, self-cleaning loads (a CPU
  spin loop, a fast disk write, and a memory-growing process) and catches them the way the
  collector would; a healthy `claude` process is correctly left alone. It is the closest
  real stand-in for `axon serve`'s collector loop until the binary is built.

  ```
  python3 scripts/axon_live_integration.py --ticks 14
  AXON_DATA_DIR=/tmp/axon-live axon savings --range last_24h   # once built: same rows
  ```
