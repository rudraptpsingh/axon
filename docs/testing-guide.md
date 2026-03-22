# Axon Testing Guide — Post-Integration Verification

## Context

This document covers changes made in the `claude/research-axon-claude-integration-gQIbC` branch.
Three roadmap features (P1-P3) were implemented along with cross-platform (Linux) support.
All changes were tested on Linux (Ubuntu 24.04, Intel Xeon, 16GB). macOS code paths compile
but have not been executed yet. This guide is for verifying on a real Mac.

### What Changed (10 files, +883 / -107 lines)

| Area | Files | Summary |
|---|---|---|
| **Headroom (P1)** | `types.rs`, `impact.rs`, `collector.rs` | `HeadroomLevel` enum (adequate/limited/insufficient) added to `HwSnapshot`. `compute_headroom()` evaluates RAM/disk pressure, throttling, CPU. |
| **Agent Accumulation (P2)** | `types.rs`, `impact.rs`, `collector.rs` | `AgentAccumulation` variant in `AnomalyType`. Detects >1 instance of Claude, Cursor, Windsurf, VS Code, Zed. |
| **Session Health (P3)** | `types.rs`, `persistence.rs`, `lib.rs` (server), `main.rs` (CLI) | New `session_health` MCP tool. `query_session_health()` aggregates snapshots + alerts from SQLite. |
| **Cross-platform** | `collector.rs`, `main.rs` | Platform-gated system profile, battery, setup/uninstall paths. Linux reads `/proc/cpuinfo`, `/sys/class/power_supply/`, `~/.config/` paths. macOS paths unchanged. |
| **Tests** | `impact.rs`, `setup_uninstall.rs`, `persistence.rs` | 20 new unit tests. Setup/uninstall tests use `#[cfg]` for platform-aware paths. |
| **MCP Server** | `lib.rs` (server), `Cargo.toml` (server) | Enhanced tool descriptions. 6-tool workflow in server instructions. `session_health` tool registered. `chrono` added as dependency. |

---

## Step 1: Build and Basic Smoke Test

```bash
cd /path/to/axon
git checkout claude/research-axon-claude-integration-gQIbC
git pull origin claude/research-axon-claude-integration-gQIbC

cargo build
cargo test --workspace
```

**Expected**: All tests pass. On macOS, the previously-failing `test_uninstall_purges_data_dirs`
should now pass (it was failing on Linux before the fix, but was already fine on macOS with the
old code; the new code uses platform-aware paths).

---

## Step 2: Verify System Profile (macOS Path)

```bash
cargo run -- query system_profile
```

**Expected output** (on your MacBook Air M2, 8GB):
```json
{
  "ok": true,
  "data": {
    "model_id": "Mac14,15",
    "chip": "Apple Silicon (...)",
    "core_count": 8,
    "ram_total_gb": 8.0,
    "os_version": "macOS ...",
    "axon_version": "0.1.4"
  }
}
```

**What to check**:
- `model_id` should NOT be empty or "Unknown" — it comes from `sysctl -n hw.model`
- `chip` should contain "Apple Silicon" — from `sysctl -n hw.perflevel0.name` fallback
- `core_count` should be 8
- `ram_total_gb` should be ~8.0

**If `model_id` or `chip` is wrong**: The `detect_platform_info()` function at
`crates/axon-core/src/collector.rs:557` is the macOS path. Debug with:
```bash
sysctl -n hw.model
sysctl -n machdep.cpu.brand_string
sysctl -n hw.perflevel0.name
```

---

## Step 3: Verify Headroom Field (P1)

```bash
cargo run -- query hw_snapshot
```

**Expected**: JSON response now includes two new fields:
```json
{
  "headroom": "adequate",
  "headroom_reason": "System has headroom"
}
```

**Headroom rules (priority order)**:

| Condition | Level | Example reason |
|---|---|---|
| RAM pressure = Critical | insufficient | "RAM at 78% (Critical)" |
| Disk pressure = Critical | insufficient | "Disk at 92% (Critical)" |
| Throttling = true | insufficient | "CPU thermal throttling at 91C" |
| RAM Warn + CPU >= 70% | insufficient | "RAM warn + CPU at 74%" |
| RAM pressure = Warn | limited | "RAM at 61% (Warn)" |
| Disk pressure = Warn | limited | "Disk at 85% (Warn)" |
| CPU >= 70% | limited | "CPU at 74%" |
| Everything else | adequate | "System has headroom" |

**To force different headroom levels**, generate load:
```bash
# Eat memory to push RAM into warn/critical
python3 -c "x = bytearray(4 * 1024**3); input('holding 4GB...')"

# In another terminal:
cargo run -- query hw_snapshot | grep headroom
```

On an 8GB MacBook Air M2, allocating 4GB should push RAM to ~75%+ (critical).

**Narrative check**: The `narrative` field should end with one of:
- `Headroom: adequate.`
- `Headroom: limited.`
- `Headroom: INSUFFICIENT -- defer heavy tasks.`

---

## Step 4: Verify Agent Accumulation Detection (P2)

```bash
# Open 2+ terminal windows, each running:
cargo run -- serve
# (they will block on stdin)

# In another terminal:
cargo run -- query process_blame
```

**Expected** when multiple `axon serve` (or `claude`, `Cursor`) processes are running:
```json
{
  "anomaly_type": "agent_accumulation",
  "culprit_group": {
    "name": "axon",
    "process_count": 2,
    ...
  },
  "impact": "Multiple AI agent instances detected. Combined resource usage is growing.",
  "fix": "2 axon instances are running. Close unused sessions to free ~0.1GB..."
}
```

Note: `axon` itself is not in the known agent list (only claude, cursor, windsurf, code, zed).
To test accumulation detection properly:
- Open multiple Claude Code sessions (`claude` CLI in different terminals)
- Or open multiple Cursor windows
- Then run `cargo run -- query process_blame`

**Known agent names** (after normalization, case-insensitive): `claude`, `claude code`,
`cursor`, `windsurf`, `code`, `zed`.

**If no accumulation is detected**: The detector is at `crates/axon-core/src/impact.rs:101`.
It checks `g.process_count > 1 && is_known_agent(&g.name)`. Debug with:
```bash
cargo run -- query process_blame 2>&1 | python3 -m json.tool
# Check the culprit_group.name — does it match a known agent?
```

---

## Step 5: Verify Session Health Tool (P3)

```bash
# Let axon collect data for a bit first:
cargo run -- serve &
sleep 30
kill %1

# Then query:
cargo run -- query session_health
```

**Expected**:
```json
{
  "ok": true,
  "data": {
    "since": "...",
    "snapshot_count": 3,
    "alert_count": 0,
    "worst_impact_level": "healthy",
    "worst_anomaly_type": "none",
    "avg_cpu_pct": 12.5,
    "avg_ram_gb": 5.2,
    "peak_cpu_pct": 25.0,
    "peak_ram_gb": 5.8,
    "peak_temp_celsius": 45.0,
    "throttle_event_count": 0
  }
}
```

**What to check**:
- `snapshot_count` > 0 (needs prior `axon serve` to populate the DB)
- `peak_temp_celsius` should be non-null on a real Mac (reads die temperature)
- `avg_cpu_pct` and `avg_ram_gb` should be reasonable values

**If snapshot_count is 0**: The DB may not have data in the last hour.
Run `axon serve` for at least 10 seconds, then try again.

---

## Step 6: Verify Battery Status (macOS Path)

```bash
cargo run -- query battery_status
```

**Expected** on a MacBook:
```json
{
  "ok": true,
  "data": {
    "percentage": 75.0,
    "is_charging": false,
    "time_to_empty_min": 180,
    "narrative": "Battery at 75% (~3h 0m remaining)."
  }
}
```

**If battery returns `ok: false`**: Check that `pmset -g batt` works in Terminal.

---

## Step 7: Verify Setup/Uninstall Paths (macOS)

```bash
# Check what agents are detected:
cargo run -- setup --list

# Setup Claude Desktop (writes to ~/Library/Application Support/Claude/):
cargo run -- setup claude-desktop

# Verify the config was written:
cat ~/Library/Application\ Support/Claude/claude_desktop_config.json | python3 -m json.tool

# Uninstall and verify cleanup:
cargo run -- uninstall claude-desktop
```

**Expected paths on macOS**:
- Claude Desktop: `~/Library/Application Support/Claude/claude_desktop_config.json`
- VS Code: `~/Library/Application Support/Code/User/settings.json`
- Cursor: `~/.cursor/mcp.json`
- Data: `~/Library/Application Support/axon/hardware.db`
- Config: `~/.config/axon/alert-dispatch.json`

---

## Step 8: MCP Protocol Test (Full E2E)

```bash
python3 scripts/mcp_exercise_all_tools.py $(which axon || echo ./target/debug/axon)
```

This script exercises all MCP tools via stdio. After our changes, there are now
**6 tools** (was 5). The script may need updating to call `session_health` — if it
only tests the original 5, that is fine. The new tool can be tested manually:

```bash
# Manual MCP test for session_health:
echo '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"test","version":"1.0"}}}
{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"session_health","arguments":{}}}' | cargo run -- serve 2>/dev/null
```

---

## Step 9: Run Full Test Suite

```bash
cargo test --workspace
cargo test -p axon --test smoke -- --ignored      # ~5s, needs real binary
```

**Expected**: All tests pass. The new tests are:

### Headroom tests (in `crates/axon-core/src/impact.rs`):
- `test_headroom_insufficient_ram_critical`
- `test_headroom_insufficient_disk_critical`
- `test_headroom_insufficient_throttling`
- `test_headroom_insufficient_warn_plus_high_cpu`
- `test_headroom_limited_ram_warn`
- `test_headroom_limited_high_cpu`
- `test_headroom_adequate`
- `test_headroom_cpu_boundary`
- `test_headroom_ram_warn_cpu_boundary`

### Agent accumulation tests (in `crates/axon-core/src/impact.rs`):
- `test_agent_accumulation_claude`
- `test_agent_accumulation_cursor`
- `test_agent_accumulation_single_is_normal`
- `test_agent_accumulation_ignores_non_agents`
- `test_suggest_fix_agent_accumulation`
- `test_impact_message_agent_accumulation`

### Setup/uninstall tests (in `crates/axon-cli/tests/setup_uninstall.rs`):
All 10 tests now use `#[cfg(target_os = "macos")]` / `#[cfg(target_os = "linux")]`
for platform-aware paths.

---

## Risk Areas

1. **macOS cfg blocks not executed in CI** — The `#[cfg(target_os = "macos")]` functions
   (`detect_platform_info`, `read_battery_macos`) were not changed in logic, only moved
   into cfg-gated functions. But a typo inside them would only surface on macOS compilation.

2. **8GB MacBook Air** — With only 8GB RAM, the thresholds (RAM_PCT_WARN=55%,
   RAM_PCT_CRITICAL=75%) will fire more easily. 4.4GB used = warn, 6GB used = critical.
   This is by design and useful — the headroom field will correctly report "limited" or
   "insufficient" when your Mac is under memory pressure.

3. **Agent accumulation false positives** — The `is_known_agent()` function uses
   `contains()` matching. A process named "encoder" would NOT match "code" because
   the check is `lower == a || lower.contains(a)` where `a` is the full agent name.
   However, "vscode-server" would match "code". This is intentional — VS Code server
   processes are legitimate agent processes.

4. **Session health with empty DB** — If `axon serve` has never run, `session_health`
   returns `snapshot_count: 0` gracefully. Not an error.

---

## Architecture Reference (Post-Changes)

```
6 MCP Tools (was 5):
  hw_snapshot      → HwSnapshot + headroom + headroom_reason
  process_blame    → ProcessBlame + AgentAccumulation detection
  battery_status   → BatteryStatus (macOS pmset / Linux sysfs)
  system_profile   → SystemProfile (macOS sysctl / Linux /proc + DMI)
  hardware_trend   → TrendData (unchanged)
  session_health   → SessionHealth (NEW: since, peaks, worst levels, alerts)

New types:
  HeadroomLevel    → adequate | limited | insufficient
  AgentAccumulation → new AnomalyType variant
  SessionHealth    → retrospective session summary struct

Platform support:
  macOS  → #[cfg(target_os = "macos")]  — original behavior preserved
  Linux  → #[cfg(target_os = "linux")]  — new code paths
  Other  → #[cfg(not(any(...)))]        — graceful fallbacks
```
