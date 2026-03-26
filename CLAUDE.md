# CLAUDE.md -- axon

## Project Overview

axon is a zero-cloud, privacy-first MCP (Model Context Protocol) server that gives AI coding agents real-time local hardware awareness. It tells developers what is slowing their machine and how to fix it -- without sending a single byte off-device. Supports macOS, Linux, and Windows.

## Architecture

```
crates/
  axon-core/     # Data types, EWMA baseline tracker, impact engine, process grouping, collector loop
  axon-server/   # MCP server (7 tools via rmcp #[tool_router])
  axon-cli/      # Binary: serve | diagnose | status | setup | query
```

- **axon-core** is a library crate. All data types live in `types.rs`. The collector loop in `collector.rs` runs every 2 seconds, refreshing sysinfo and updating per-process EWMA baselines. Process grouping in `grouping.rs` aggregates child processes by app name (e.g., Chrome helpers → "Google Chrome").
- **axon-server** exposes 7 MCP tools over stdio: `hw_snapshot`, `process_blame`, `battery_status`, `system_profile`, `hardware_trend`, `session_health`, `gpu_snapshot`. Uses rmcp 1.x with `#[tool_router]` and `#[tool_handler]` macros.
- **axon-cli** is the binary entry point (package name `axon`). Agent setup is explicit via `axon setup` (supports claude-desktop, claude-code, cursor, vscode).

## Key Technical Details

- **rmcp quirk**: `serve()` returns a `RunningService` handle. You MUST call `.waiting().await` on it or the server exits immediately after initialization. This was a hard-won lesson.
- **rmcp exits on stdin EOF**: If the MCP client sends no `initialize` request, rmcp returns `Error: connection closed: initialize request` from `serve()`. Any code after `serve().await?` is never reached. Never put critical logic (e.g. alert persistence) solely inside `run_server` — it will be skipped when there is no MCP handshake.
- **stdio contract**: stdout is reserved exclusively for MCP JSON-RPC. All logging goes to stderr via `tracing`. Never `println!` from the server path.
- **Claude Desktop PATH**: Claude Desktop's subprocess PATH is limited to system directories. Always use absolute binary paths in `claude_desktop_config.json`.
- **sysinfo 0.33**: `component.temperature()` returns `Option<f32>`, not `f32`.
- **EWMA**: Three timescales per process — fast (α=0.4, ~5s), medium (α=0.2, ~10s), slow (α=0.05, ~40s). Each uses an Adaptive EWMA (Capizzi & Masarotto 2003) with Huber score to resist baseline drift during sustained anomalies. Warmup: fast needs 2 samples, medium 3, slow 8. The slow delta drives `ram_growth_gb_per_sec` and `rss_growth_rate_mb_per_hr`. See `crates/axon-core/src/ewma.rs`.
- **Impact / alert thresholds**: Tunable in `crates/axon-core/src/thresholds.rs` (RAM warn/critical %, thermal °C, anomaly classification, impact score bands, persistence sample count). Lower values trigger sooner.
- **No network calls**: This is a core design constraint. Never add telemetry, analytics, or any outbound network activity.
- **Alert dispatch config**: Default path is `~/.config/axon/alert-dispatch.json`. Set **`AXON_CONFIG_DIR`** to a directory to load `<dir>/alert-dispatch.json` instead, or pass **`axon serve --config-dir <dir>`** (CLI wins over the env var). **`--alert-webhook ID=URL`** and **`--alert-filter channel.key=value`** merge into the loaded file config (see `axon_core::alert_config::apply_cli_overrides`).
- **Alert triggers and consumption**: Alerts are **edge-triggered** (RAM/throttle/impact transitions), not periodic pings. **Persistence is in the collector** (`collector.rs`): alerts are inserted into SQLite the moment they are detected, independent of any MCP connection. `alert_sender` in `axon-server` only handles webhook dispatch and MCP logging notifications (`dispatch_webhooks_only`). **Webhooks**: add a `webhook`-type channel in `alert-dispatch.json`; Axon POSTs JSON (`WebhookPayload`) to the URL (fire-and-forget). To consume locally, run `python3 scripts/alert_receiver_minimal.py` and paste the printed URL into config, then reload MCP. **MCP**: eligible alerts also use `notifications/message` (logging), which many clients do not surface prominently—prefer webhooks for reliable delivery. Proof of POST + filters: `cargo test -p axon-core --test alert_integration`. Live machine runs may see zero webhooks if nothing transitions; use `ALERT_E2E_WAIT` with `scripts/test_alert_webhooks_live.py` or generate load.
- **Alert state injection for tests**: Set `AXON_TEST_PREV_RAM_PRESSURE`, `AXON_TEST_PREV_IMPACT_LEVEL`, `AXON_TEST_PREV_THROTTLING` to inject previous state into the collector (forces a known edge transition on tick 4). Set `AXON_TEST_PRESERVE_PREV_DURING_WARMUP=1` to hold those injected values through the 3-tick warm-up window.
- **GPU monitoring**: Implemented in `crates/axon-core/src/gpu.rs`. macOS reads `ioreg -r -c IOAccelerator` (no sudo). Linux tries `nvidia-smi` first, then AMD sysfs (`/sys/class/drm/cardN/device/gpu_busy_percent`, `mem_info_vram_used`, `mem_info_vram_total`). Windows tries `nvidia-smi` first (NVIDIA GPUs), then falls back to GPU Engine performance counters (real-time utilization for AMD/Intel/NVIDIA) combined with WMI `Win32_VideoController` (model name + total VRAM). GPU static info is cached; utilization is refreshed every 5 ticks (~10s) to avoid PowerShell startup overhead. `GpuSnapshot.detected` is `false` when no GPU is found; the narrative will say "No GPU detected" rather than returning all-null fields silently. The collector always stores the snapshot (never skips it) so `detected=false` reaches the MCP layer. Unit tests for the nvidia-smi CSV parser run on Linux and Windows without hardware; live tests are gated behind `--ignored`.
- **Claude/Cursor issue detection signals**: The collector detects 20+ failure patterns derived from open GitHub issues in anthropics/claude-code. Signals live in two structs: `ClaudeAgentInfo` (per-process) and `HwSnapshot` (system-wide). Sampling cadence: most signals fire every tick (2s); `dot_claude_size_gb` and `large_session_file_mb` are sampled every 30 ticks (~60s) to amortize filesystem overhead. Key signals and their issue references:
  - `child_churn_rate_per_sec` — zombie storm (#34092): parent spawning >20 children/tick
  - `io_read_mb_per_sec` — polling/re-read loop (#22543): >50 MB/s reads with low CPU
  - `idle_cpu_spin_secs` — futex/pread busy-wait: CPU >30% with no children and no I/O for >60s
  - `rss_growth_rate_mb_per_hr` — node-pty ArrayBuffer leak (#31511, #33118): EWMA growth >50 MB/hr
  - `system_fd_pct` — inotify watcher exhaustion (#11136): `/proc/sys/fs/file-nr` pool >85%
  - `oom_freeze_risk` — Linux hard freeze: MemFree+SwapFree <64MB with SwapFree=0
  - `large_session_file_mb` — sync load hang (#21022): largest `.jsonl` >40MB
  - `bun_crash_trajectory` — mimalloc OOM (#21875, #29192): uptime >4h AND growth >300 MB/hr
  - `dot_claude_size_gb` — runaway logs/cache (#16093, #26911): ~/.claude/ total size
  - `mcp_server_count` — commit charge drain: count of running MCP server processes
  - `stale_session_count` — invisible wait states: claude PIDs with >24h uptime and >200MB RAM
  - `zombie_child_count` — per-PID zombie children (complement to churn rate)
  - `subagent_orphan_count_total` — all PPID=1 claude/bun (broadens `orphan_pids`)
- **Collector helper functions** (Linux-only unless noted): `read_system_fd_pct()` reads `/proc/sys/fs/file-nr`; `check_oom_freeze_risk()` reads `/proc/meminfo`; `read_pid_io_bytes(pid)` reads `/proc/<pid>/io`; `read_dot_claude_size_gb()` walks `~/.claude/` (all platforms); `count_mcp_servers(sys)` scans process cmdlines (all platforms); `largest_session_file_mb(session_id)` globs `~/.claude/projects/**/*.jsonl` (all platforms).
- **Per-tick state maps in collector**: `prev_child_counts`, `prev_io_read_bytes`, `idle_spin_ticks` are evicted each tick alongside `agent_idle_ticks` and `agent_d_state_ticks` using `retain(|pid,_| active_pids.contains(pid))`. All are keyed by claude PID and bounded to the live process set.
- **Memory footprint**: Measured on Linux debug build — VmRSS **4.6 MB** steady state. RSS does not grow over time because `SnapshotRing` uses `VecDeque::with_capacity(1800)` (pre-allocates the full 1h ring at startup). Breakdown: ring buffer ~750 KB, EWMA store ~43 KB (200 PIDs), sysinfo System ~600 KB, SQLite WAL ~700 KB, Tokio runtime ~750 KB. Comparable to collectd (~5–15 MB); 7–10× lighter than Prometheus node_exporter (~25 MB); 40–80× lighter than Netdata (~100–150 MB). Any Python/Node.js/Bun MCP server alternative carries a 20–43 MB runtime floor before monitoring logic runs.

## Build & Test

```bash
cargo build                                # Debug build
cargo test --workspace                     # Library + CLI tests (run counts vary)
cargo test -p axon --test smoke -- --ignored   # ~5s: real diagnose + status binaries
cargo install --path crates/axon-cli       # Install to ~/.cargo/bin
axon diagnose                              # Quick smoke test
python3 scripts/mcp_exercise_all_tools.py /path/to/axon  # Exercise all 7 MCP tools
cargo test -p axon-core --lib gpu          # GPU parser unit tests (no hardware needed)
cargo test -p axon-core --lib gpu -- --ignored --nocapture  # Live GPU smoke test (requires GPU)
```
Live webhook E2E (needs release binary; may wait up to `ALERT_E2E_WAIT` seconds): `scripts/e2e-webhook-config-file.sh`, `scripts/e2e-webhook-cli-override.sh`, `scripts/e2e-mcp-and-webhook.sh`.
- **Live stress test (ignored)**: `cargo test -p axon --test live_hardware_alert -- --ignored --nocapture` — real `axon serve`, baseline RAM from `axon_core::probe`, memory hog + `yes` CPU stress, asserts webhook or new `alerts` rows. Works on machines with Critical baseline RAM via injected prev-state env vars (edge transition guaranteed on tick 4). Typically completes in ~5 minutes.
- **Mock alert state-machine tests** (no hardware needed): `cargo test -p axon-core --test alert_integration` — includes `test_alert_full_collector_cycle_mock` and related tests that verify edge-trigger invariants in memory.

## Code Conventions

- **No emojis** in output, logs, or documentation. Use text-based indicators: `[ok]`, `[warn]`, `[err]`, `[info]`.
- **Minimal dependencies**: Do not add crates without justification. Prefer std where possible.
- **Error handling**: Use `anyhow::Result` for CLI/application code. MCP tool handlers return serialized JSON strings, never panic.
- **Naming**: snake_case for functions/variables, PascalCase for types. Tool names use snake_case to match MCP convention.
- **Tests**: Unit tests go in the same file (`#[cfg(test)] mod tests`). Integration tests go in `tests/` directories.
- **Commits**: Imperative mood, concise first line, body explains "why" not "what".

## MCP Tool Signatures

5 tools take `EmptyParams` (no arguments): `hw_snapshot`, `process_blame`, `battery_status`, `system_profile`, `gpu_snapshot`. `hardware_trend` accepts `TrendParams { time_range: Option<String>, interval: Option<String> }`. `session_health` accepts `SessionHealthParams { since: Option<String> }` (ISO 8601 timestamp, defaults to 1 hour ago). All return a JSON string wrapped in `McpResponse<T>`:
```json
{"ok": true, "ts": "...", "data": {...}, "narrative": "human-readable summary"}
```
`gpu_snapshot` always returns `ok: true`. When no GPU is present `data.detected` is `false` and the narrative explains why (e.g. "No GPU detected on this system. nvidia-smi not found and no DRM sysfs device present.").

### Notable fields exposed to agents

**`hw_snapshot` → `HwSnapshot`** (system-wide):
- `system_fd_pct` — system FD pool % (Linux); fires narrative at >85%
- `oom_freeze_risk` — true when MemFree+SwapFree <64MB and no swap (Linux)
- `dot_claude_size_gb` — ~/.claude/ total size in GB; sampled every ~60s
- `mcp_server_count` — count of running MCP server processes
- `disk_fill_rate_gb_per_sec` — active fill rate; fires at >50 MB/s
- `swap_used_gb` / `swap_total_gb` — swap pressure
- `irq_per_sec` — hardware interrupt rate for spin-loop vs I/O distinction (Linux)

**`process_blame` → `ProcessBlame`** (system + per-agent):
- `claude_agents: Vec<ClaudeAgentInfo>` — per-process breakdown including all signals below
- `stale_session_count` — claude PIDs with >24h uptime and >200 MB RAM
- `subagent_orphan_count_total` — all PPID=1 claude/bun (includes idle orphans)
- `orphan_pids` / `zombie_pids` / `crashed_agent_pids` / `stranded_idle_pids`

**`ClaudeAgentInfo`** fields (per running claude process):
- `child_churn_rate_per_sec`, `zombie_child_count` — subprocess storm detection
- `io_read_mb_per_sec` — disk polling loop detection (Linux)
- `idle_cpu_spin_secs` — sustained CPU burn with no real work
- `rss_growth_rate_mb_per_hr` — memory leak rate (early warning before gc_pressure)
- `gc_pressure` — "warn" (>800 MB) / "critical" (>1.5 GB) / "accumulating"
- `large_session_file_mb` — session file >40 MB (sync load hang risk)
- `bun_crash_trajectory` — uptime >4h AND growth >300 MB/hr (imminent OOM)
- `fd_leak` — FDSize >4096 (inotify watcher leak, EMFILE risk)
- `suspected_spin_loop`, `suspected_alloc_thrash`, `suspected_io_block`
- `ram_spike` — single-tick RAM jump >300 MB above fast EWMA baseline
- `uptime_s` — estimated session age from EWMA sample count

## Agent Setup Targets

The CLI supports: `claude-desktop`, `claude-code`, `cursor`, `vscode`. Each writes to the agent's config file using the appropriate JSON structure. Setup is explicit via `axon setup [target]`.

## What NOT To Do

- Do not add network calls or telemetry of any kind
- Do not write to stdout from the server path (breaks MCP JSON-RPC)
- Do not use `std::sync::Mutex` in async code paths without careful consideration (current usage is safe because locks are held briefly)
- Do not add fleet/team APIs yet (Phase 3 — requires privacy model rethink)
- Do not hold a second borrow on `sys.processes()` inside the `claude_agents` filter_map closure — pre-compute lookup maps (e.g. `direct_child_counts`, `zombie_child_counts_map`) before the closure to avoid lifetime conflicts
- Do not call expensive filesystem operations (directory walks, glob) every tick — gate them with `tick_count % 30 == 1` and cache results between samples (see `cached_dot_claude_size_gb`, `large_session_file_mb` pattern)
- Do not run `axon serve` without an MCP client on stdin — rmcp exits immediately with "connection closed: initialize request" when stdin reaches EOF (use a named pipe `/tmp/axon_stdin_pipe` to keep stdin open for testing)
