# Axon Roadmap

Each item below is grounded in a specific, documented developer pain point.
The "Problem" section gives an AI coding agent enough context to understand
*why* a change exists before touching any code. The "Test Scenarios" section
defines what must be verified after implementation.

---

## Item 1: Multi-Instance Agent Process Detection

### Status: Done (v0.2.0)

### Problem this solves

When developers run multiple Claude Code, Cursor, or Windsurf sessions
simultaneously, the processes accumulate silently. Each instance consumes
270-370MB RAM and 1.7-2.2% CPU. The compounding effect causes thermal
throttling and system slowdowns that users attribute to other causes.

Documented in `anthropics/claude-code` GitHub issues:
- Issue #11122: Multiple Claude CLI processes accumulate, high CPU, no visual
  indicator that multiple instances are running.
- Issue #18859: 4 idle Claude Code sessions consumed 60GB total. macOS OOM
  crash. No mechanism detected or warned about the accumulation.
- Issue #24960: 3 Claude processes hit 17.3GB on an 18GB machine. Kernel panic.

Current behavior: `process_blame` returns the top culprit by `blame_score`.
If 4 Claude processes are running, it returns the one with the highest score
but does not identify "you have 4 instances of Claude running" as its own
named problem. The fix suggestion does not address accumulation.

### What to build

Add `AgentAccumulation` as a new variant to `AnomalyType` in
`crates/axon-core/src/types.rs`:

```rust
pub enum AnomalyType {
    None,
    MemoryPressure,
    CpuSaturation,
    ThermalThrottle,
    GeneralSlowdown,
    AgentAccumulation,   // <-- new
}
```

In `crates/axon-core/src/grouping.rs`, after `build_groups` produces its
output, add a post-processing step that checks the resulting groups for known
agent process names with `process_count > 1`. Known agent group names to
detect (after `normalize_process_name`): `"claude"`, `"Claude Code"`,
`"Cursor"`, `"Windsurf"`, `"code"` (VS Code).

In `crates/axon-core/src/impact.rs`, update `suggest_fix` to handle the
`AgentAccumulation` anomaly type with a fix that names the agent and count:

```
"3 Claude Code instances are running. Close unused terminals to free ~1.1GB
and reduce background CPU."
```

In `crates/axon-core/src/impact.rs`, update `detect_anomaly_type` signature
or add a separate `detect_agent_accumulation(groups: &[ProcessGroup]) -> bool`
check so the collector can classify it. `AgentAccumulation` should take
priority after `ThermalThrottle` in the classification order.

In `crates/axon-core/src/impact.rs`, update `impact_message` to handle
`(_, AnomalyType::AgentAccumulation)` cases.

### Files to change

- `crates/axon-core/src/types.rs` -- add `AgentAccumulation` to `AnomalyType`
- `crates/axon-core/src/grouping.rs` -- add agent name detection list and
  `detect_agent_accumulation(groups: &[ProcessGroup]) -> Option<&ProcessGroup>`
- `crates/axon-core/src/impact.rs` -- update `detect_anomaly_type` or add
  separate detection, update `suggest_fix` and `impact_message`
- `crates/axon-core/src/collector.rs` -- integrate new detection into the
  `ProcessBlame` construction path

### Test scenarios

**Unit tests in `crates/axon-core/src/grouping.rs`:**

1. `test_agent_accumulation_claude` -- build_groups with 3 processes named
   "claude" (different PIDs). Assert that a downstream accumulation check on
   the resulting groups identifies "claude" with process_count=3.

2. `test_agent_accumulation_cursor` -- 5 Cursor Helper (Renderer) processes.
   After normalize_process_name they all become "Cursor". Assert detection
   fires on process_count=5.

3. `test_agent_accumulation_single_instance` -- 1 claude process. Assert
   detection does NOT fire (single instance is normal).

4. `test_agent_accumulation_ignores_non_agents` -- 10 "node" processes. Assert
   detection does not fire (node is not a known agent process name).

**Unit tests in `crates/axon-core/src/impact.rs`:**

5. `test_suggest_fix_agent_accumulation_claude` -- culprit_group name "claude",
   process_count=3, anomaly AgentAccumulation. Assert fix string contains "3"
   and "Claude".

6. `test_suggest_fix_agent_accumulation_cursor` -- culprit_group name "Cursor",
   process_count=4. Assert fix contains "4" and "Cursor".

7. `test_impact_message_agent_accumulation` -- Assert impact_message returns a
   non-empty, non-"No action needed" string for AgentAccumulation.

**Integration behavior to verify manually:**

8. Run `axon diagnose` with 2+ terminal sessions each running `axon serve`.
   Assert the culprit or narrative mentions the accumulation.

---

## Item 2: Headroom Field in hw_snapshot

### Status: Done (v0.2.0)

### Problem this solves

Agents need to pre-check "can I start this heavy task?" before launching
`cargo test --workspace`, a Docker build, or a large code generation pass.
Currently `hw_snapshot` provides `ram_pressure`, `disk_pressure`,
`cpu_usage_pct`, and `throttling` as separate fields. The agent must interpret
all four together and reason about the combined picture.

This overhead is documented in:
- METR study (2025): 19% developer slowdown partly attributed to cognitive
  overhead of gathering context that should be automatic.
- GitHub issue #17563: MacBook Air M4 OOM during Claude Code session. An
  agent that pre-checked headroom could have warned: "Insufficient headroom --
  do not start this task."
- GitHub issue #9897: "Claude Code using massive amounts of memory and heating
  up my computer." Pre-task headroom check would have gated the task.

Current behavior: An agent must write logic like: "if ram_pressure == critical
OR disk_pressure == critical OR (throttling == true AND cpu_usage_pct > 80)
then warn user." This is agent-side reasoning that should be pre-computed.

### What to build

Add a `HeadroomLevel` enum and `headroom_reason` string to `HwSnapshot` in
`crates/axon-core/src/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum HeadroomLevel {
    Adequate,       // safe to start heavy tasks
    Limited,        // proceed with caution, monitor closely
    Insufficient,   // do not start heavy tasks
}

pub struct HwSnapshot {
    // ... existing fields unchanged ...
    pub headroom: HeadroomLevel,
    pub headroom_reason: String,  // e.g. "RAM at 78% (Critical)"
}
```

Add a `compute_headroom(snap: &HwSnapshot) -> (HeadroomLevel, String)` function
in `crates/axon-core/src/impact.rs` using these rules (in priority order):

| Condition | Level | Example reason string |
|---|---|---|
| ram_pressure == Critical OR disk_pressure == Critical | Insufficient | "RAM at 78% (Critical)" |
| throttling == true | Insufficient | "CPU thermal throttling at 91C" |
| ram_pressure == Warn AND cpu_usage_pct > 70.0 | Insufficient | "RAM warn + CPU at 74%" |
| ram_pressure == Warn OR disk_pressure == Warn | Limited | "RAM at 61% (Warn)" |
| cpu_usage_pct > 70.0 | Limited | "CPU at 74%" |
| (all else) | Adequate | "System has headroom" |

Update wherever `HwSnapshot` is constructed (primarily
`crates/axon-core/src/collector.rs`) to populate the two new fields by calling
`compute_headroom`.

The `hardware_trend` tool reads from SQLite snapshots. The `headroom` and
`headroom_reason` fields are computed at snapshot time, not stored -- they are
derived from the other fields. No schema change needed.

### Files to change

- `crates/axon-core/src/types.rs` -- add `HeadroomLevel` enum, add `headroom`
  and `headroom_reason` fields to `HwSnapshot`
- `crates/axon-core/src/impact.rs` -- add `compute_headroom` function
- `crates/axon-core/src/collector.rs` -- call `compute_headroom` when building
  `HwSnapshot`
- Update any test fixtures that construct `HwSnapshot` directly

### Test scenarios

**Unit tests in `crates/axon-core/src/impact.rs`:**

1. `test_headroom_insufficient_ram_critical` -- ram_pressure=Critical,
   disk_pressure=Normal, throttling=false, cpu=50. Assert Insufficient, reason
   contains "RAM" and "Critical".

2. `test_headroom_insufficient_disk_critical` -- disk_pressure=Critical, RAM
   normal. Assert Insufficient, reason contains "Disk" and "Critical".

3. `test_headroom_insufficient_throttling` -- throttling=true, both pressures
   Normal. Assert Insufficient, reason mentions throttling or temperature.

4. `test_headroom_insufficient_warn_plus_high_cpu` -- ram_pressure=Warn,
   cpu_usage_pct=75.0. Assert Insufficient.

5. `test_headroom_limited_ram_warn` -- ram_pressure=Warn, cpu=40, no
   throttling, disk Normal. Assert Limited.

6. `test_headroom_limited_high_cpu` -- ram Normal, cpu_usage_pct=72.0. Assert
   Limited.

7. `test_headroom_adequate` -- ram Normal, disk Normal, throttling false,
   cpu=30. Assert Adequate, reason contains "headroom".

8. `test_headroom_reason_is_nonempty` -- all inputs. Assert headroom_reason is
   never an empty string.

**Boundary tests:**

9. `test_headroom_cpu_boundary` -- cpu_usage_pct=69.9 → Adequate.
   cpu_usage_pct=70.0 → Limited.

10. `test_headroom_ram_warn_boundary` -- ram_pressure=Warn, cpu=69.9 →
    Limited. ram_pressure=Warn, cpu=70.0 → Insufficient.

**Integration:**

11. Run `axon query hw_snapshot`. Assert the JSON response contains
    `"headroom"` and `"headroom_reason"` fields. Assert `headroom` is one of
    `"adequate"`, `"limited"`, `"insufficient"` (snake_case via serde).

12. Run `axon status`. Confirm headroom is surfaced in the output.

---

## Item 3: Session-Scoped Health Summary

### Status: Done (v0.2.0)

### Problem this solves

Long agentic sessions (multi-hour refactors, large test runs) degrade
gradually. Edge-triggered alerts fire on transitions but give no retrospective
view of "what happened during this session." An agent that runs for 6 hours
cannot ask "how has the machine behaved since I started?" without querying
`hardware_trend` and reasoning about raw buckets.

Documented in:
- GitHub issue #11377: Claude process ran for 14 hours, consumed 23GB RAM /
  143% CPU. No periodic health summary existed that could have surfaced the
  accumulating problem.
- GitHub issue #18859: 4 sessions accumulated 60GB over 18 hours. An agent
  that could query "since session start, how many alerts have fired and what
  was the worst state?" would have surfaced this.
- METR study: Much of the 19% developer slowdown comes from cognitive overhead
  of gathering context that should be automatic. Session health is exactly this.

Current behavior: `hardware_trend` returns bucketed CPU/RAM/temp/anomaly
averages over a fixed time window (last_1h, last_6h, etc.). An agent must
choose a window, receive N buckets, and compute the worst state itself. There
is no "give me the summary since session start (a specific timestamp)" API, and
no pre-computed "worst state" field.

### What to build

Add a new MCP tool `session_health` in `crates/axon-server/src/tools.rs`.

The tool accepts a single optional parameter `since: Option<String>` (ISO 8601
timestamp). If `since` is omitted, defaults to 1 hour ago. The tool queries
SQLite for all snapshots and alerts since that timestamp and returns a
`SessionHealth` struct.

Add `SessionHealth` to `crates/axon-core/src/types.rs`:

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionHealth {
    pub since: DateTime<Utc>,
    pub snapshot_count: u32,
    pub alert_count: u32,
    pub worst_impact_level: ImpactLevel,
    pub worst_anomaly_type: AnomalyType,
    pub avg_anomaly_score: f64,
    pub avg_cpu_pct: f64,
    pub avg_ram_pct: f64,
    pub peak_cpu_pct: f64,
    pub peak_ram_pct: f64,
    pub peak_temp_celsius: Option<f64>,
    pub throttle_event_count: u32,
}
```

Add a `query_session_health(since: DateTime<Utc>) -> anyhow::Result<SessionHealth>`
function to `crates/axon-core/src/persistence.rs`. It runs a single SQL query
over the `snapshots` table (and `alerts` table for `alert_count` and
`throttle_event_count`).

The `worst_impact_level` ordering for SQL aggregation: map
Healthy=0, Degrading=1, Strained=2, Critical=3. Store as integer in SQLite,
return the MAX. If SQLite schema does not currently store `impact_level` as an
integer, add the column in a migration.

Update `crates/axon-server/src/tools.rs` to register the new tool using the
existing `#[tool_router]` / `#[tool_handler]` pattern.

Update `crates/axon-cli/src/main.rs` (the `query` subcommand) to accept
`session_health` as a valid tool name.

### Files to change

- `crates/axon-core/src/types.rs` -- add `SessionHealth` struct
- `crates/axon-core/src/persistence.rs` -- add `query_session_health` function;
  add `impact_level` integer column to `snapshots` table if not present
- `crates/axon-server/src/tools.rs` -- register `session_health` tool with
  `since` parameter, call `query_session_health`, wrap in `McpResponse`
- `crates/axon-cli/src/main.rs` -- add `session_health` to `query` subcommand
  valid tool names

### Test scenarios

**Unit tests in `crates/axon-core/src/persistence.rs`:**

1. `test_session_health_empty_window` -- query a window with no snapshots.
   Assert snapshot_count=0, alert_count=0, worst_impact_level=Healthy,
   avg_anomaly_score=0.0.

2. `test_session_health_single_snapshot` -- insert one snapshot. Assert
   snapshot_count=1, avg_cpu_pct matches the inserted value,
   avg_ram_pct matches.

3. `test_session_health_worst_impact_level` -- insert 3 snapshots with impact
   levels Healthy, Strained, Degrading. Assert worst_impact_level=Strained.

4. `test_session_health_worst_anomaly_type` -- insert snapshots with anomaly
   types None, MemoryPressure, ThermalThrottle. Assert worst_anomaly_type is
   the most severe observed (ThermalThrottle).

5. `test_session_health_alert_count` -- insert 3 alerts in the window and 2
   outside the window. Assert alert_count=3.

6. `test_session_health_throttle_events` -- insert 2 snapshots with
   throttling=true, 3 with throttling=false. Assert throttle_event_count=2.

7. `test_session_health_peak_values` -- insert snapshots with varying
   cpu_usage_pct. Assert peak_cpu_pct = the maximum observed value.

8. `test_session_health_since_filters_correctly` -- insert snapshots at T-2h
   and T-30m. Query with since=T-1h. Assert only the T-30m snapshot is counted.

**Integration:**

9. Run `axon query session_health`. Assert the JSON response contains all
   `SessionHealth` fields. Assert `ok: true`.

10. Run `axon query session_health` with `since` set to a future timestamp.
    Assert snapshot_count=0 and ok=true (empty window is not an error).

11. Run `axon serve` as an MCP server and call `session_health` via the MCP
    protocol (using `scripts/mcp_exercise_all_tools.py` once updated). Assert
    the tool is listed in `tools/list` and returns a valid response.

---

## Item 4: Linux and Windows Support

### Status: Linux done (v0.2.0), Windows planned

### Problem this solves

The documented hardware failures (GitHub issues #17563, #11615, #9897, #18859,
#24960) are not exclusive to macOS. Claude Code, Cursor, and VS Code are heavily
used on Linux developer machines and Windows laptops. Developers on those
platforms have no hardware-aware MCP tool.

The underlying `sysinfo` crate (used in `crates/axon-core/src/collector.rs`)
already supports Linux and Windows. The platform gap is in:
- Temperature reading: `sysinfo` component temperature support varies by
  platform. Linux uses hwmon sensors. Windows uses WMI or OpenHardwareMonitor.
- Battery: `battery` crate (if used) or `sysinfo` battery support.
- Process name normalization: `normalize_process_name` in
  `crates/axon-core/src/grouping.rs` strips macOS-specific helper suffixes.
  Linux process names have different patterns.
- Agent setup paths: `axon setup` writes to macOS-specific config file
  locations.

### What to build

This is tracked but not fully specified yet. The primary work is:

1. Audit every `#[cfg(target_os = "macos")]` block in `axon-core` and
   `axon-cli` for completeness.
2. Test `sysinfo` component temperature on Linux (hwmon) and Windows (WMI).
3. Extend `normalize_process_name` for Linux process name patterns (no
   `.app` bundles, no `(Renderer)` suffix on all helpers).
4. Update `axon setup` config file paths for Linux
   (`~/.config/Claude/claude_desktop_config.json`) and Windows equivalents.
5. CI: add Linux runner to `.github/workflows`.

### Test scenarios (deferred until implementation is specified)

- `axon diagnose` runs to completion on Ubuntu 22.04 LTS.
- `axon diagnose` runs to completion on Windows 11.
- `hw_snapshot` returns `die_temp_celsius: null` gracefully when no
  temperature sensor is available (not an error, just `None`).
- `axon setup claude-desktop` writes the correct path on Linux.

---

## Prioritization

| Item | Impact | Effort | Priority |
|---|---|---|---|
| Item 2: Headroom field in hw_snapshot | High -- eliminates agent-side reasoning overhead for most common pre-task check | Low -- new field on existing struct, one function | 1 |
| Item 1: Multi-instance agent detection | High -- directly addresses documented OOM/crash scenarios | Medium -- new AnomalyType variant, grouping post-pass | 2 |
| Item 3: Session-scoped health summary | Medium -- improves long-session agent behavior | Medium -- new tool, DB query | 3 |
| Item 4: Linux/Windows support | High (market reach) | High (platform testing) | 4 |

---

## What is NOT on this roadmap

These were considered and explicitly excluded:

- **GPU monitoring**: Requires platform-specific APIs (Metal Performance Shaders
  on macOS, NVML on Linux/Windows). Complex. Phase 3.
- **Fleet/team APIs**: Requires privacy model rethink. Axon is zero-cloud by
  design. Any multi-machine feature must maintain that constraint. Phase 3.
- **Cloud telemetry or analytics**: Permanent exclusion. Core design constraint.
  See `CLAUDE.md`.
- **Network-based alert delivery beyond webhooks**: Webhooks are fire-and-forget
  to a user-controlled endpoint. No managed cloud relay.
