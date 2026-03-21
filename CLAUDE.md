# CLAUDE.md -- axon

## Project Overview

axon is a zero-cloud, privacy-first MCP (Model Context Protocol) server that gives AI coding agents real-time local hardware awareness. It tells developers what is slowing their machine and how to fix it -- without sending a single byte off-device. Currently targets macOS; Linux and Windows support planned.

## Architecture

```
crates/
  axon-core/     # Data types, EWMA baseline tracker, impact engine, process grouping, collector loop
  axon-server/   # MCP server (5 tools via rmcp #[tool_router])
  axon-cli/      # Binary: serve | diagnose | status | setup | query
```

- **axon-core** is a library crate. All data types live in `types.rs`. The collector loop in `collector.rs` runs every 2 seconds, refreshing sysinfo and updating per-process EWMA baselines. Process grouping in `grouping.rs` aggregates child processes by app name (e.g., Chrome helpers → "Google Chrome").
- **axon-server** exposes 5 MCP tools over stdio: `hw_snapshot`, `process_blame`, `battery_status`, `system_profile`, `hardware_trend`. Uses rmcp 1.x with `#[tool_router]` and `#[tool_handler]` macros.
- **axon-cli** is the binary entry point (package name `axon`). Agent setup is explicit via `axon setup` (supports claude-desktop, claude-code, cursor, vscode).

## Key Technical Details

- **rmcp quirk**: `serve()` returns a `RunningService` handle. You MUST call `.waiting().await` on it or the server exits immediately after initialization. This was a hard-won lesson.
- **rmcp exits on stdin EOF**: If the MCP client sends no `initialize` request, rmcp returns `Error: connection closed: initialize request` from `serve()`. Any code after `serve().await?` is never reached. Never put critical logic (e.g. alert persistence) solely inside `run_server` — it will be skipped when there is no MCP handshake.
- **stdio contract**: stdout is reserved exclusively for MCP JSON-RPC. All logging goes to stderr via `tracing`. Never `println!` from the server path.
- **Claude Desktop PATH**: Claude Desktop's subprocess PATH is limited to system directories. Always use absolute binary paths in `claude_desktop_config.json`.
- **sysinfo 0.33**: `component.temperature()` returns `Option<f32>`, not `f32`.
- **EWMA**: alpha=0.2, requires 3+ samples before reporting deltas.
- **Impact / alert thresholds**: Tunable in `crates/axon-core/src/thresholds.rs` (RAM warn/critical %, thermal °C, anomaly classification, impact score bands, persistence sample count). Lower values trigger sooner.
- **No network calls**: This is a core design constraint. Never add telemetry, analytics, or any outbound network activity.
- **Alert dispatch config**: Default path is `~/.config/axon/alert-dispatch.json`. Set **`AXON_CONFIG_DIR`** to a directory to load `<dir>/alert-dispatch.json` instead, or pass **`axon serve --config-dir <dir>`** (CLI wins over the env var). **`--alert-webhook ID=URL`** and **`--alert-filter channel.key=value`** merge into the loaded file config (see `axon_core::alert_config::apply_cli_overrides`).
- **Alert triggers and consumption**: Alerts are **edge-triggered** (RAM/throttle/impact transitions), not periodic pings. **Persistence is in the collector** (`collector.rs`): alerts are inserted into SQLite the moment they are detected, independent of any MCP connection. `alert_sender` in `axon-server` only handles webhook dispatch and MCP logging notifications (`dispatch_webhooks_only`). **Webhooks**: add a `webhook`-type channel in `alert-dispatch.json`; Axon POSTs JSON (`WebhookPayload`) to the URL (fire-and-forget). To consume locally, run `python3 scripts/alert_receiver_minimal.py` and paste the printed URL into config, then reload MCP. **MCP**: eligible alerts also use `notifications/message` (logging), which many clients do not surface prominently—prefer webhooks for reliable delivery. Proof of POST + filters: `cargo test -p axon-core --test alert_integration`. Live machine runs may see zero webhooks if nothing transitions; use `ALERT_E2E_WAIT` with `scripts/test_alert_webhooks_live.py` or generate load.
- **Alert state injection for tests**: Set `AXON_TEST_PREV_RAM_PRESSURE`, `AXON_TEST_PREV_IMPACT_LEVEL`, `AXON_TEST_PREV_THROTTLING` to inject previous state into the collector (forces a known edge transition on tick 4). Set `AXON_TEST_PRESERVE_PREV_DURING_WARMUP=1` to hold those injected values through the 3-tick warm-up window.

## Build & Test

```bash
cargo build                                # Debug build
cargo test --workspace                     # Library + CLI tests (run counts vary)
cargo test -p axon --test smoke -- --ignored   # ~5s: real diagnose + status binaries
cargo install --path crates/axon-cli       # Install to ~/.cargo/bin
axon diagnose                              # Quick smoke test
python3 scripts/mcp_exercise_all_tools.py /path/to/axon  # Exercise all 5 MCP tools
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

4 tools take `EmptyParams` (no arguments). `hardware_trend` accepts `TrendParams { time_range: Option<String>, interval: Option<String> }`. All return a JSON string wrapped in `McpResponse<T>`:
```json
{"ok": true, "ts": "...", "data": {...}, "narrative": "human-readable summary"}
```

## Agent Setup Targets

The CLI supports: `claude-desktop`, `claude-code`, `cursor`, `vscode`. Each writes to the agent's config file using the appropriate JSON structure. Setup is explicit via `axon setup [target]`.

## What NOT To Do

- Do not add network calls or telemetry of any kind
- Do not write to stdout from the server path (breaks MCP JSON-RPC)
- Do not use `std::sync::Mutex` in async code paths without careful consideration (current usage is safe because locks are held briefly)
- Do not add GPU monitoring yet (Phase 3 — complex, platform-specific)
- Do not add fleet/team APIs yet (Phase 3 — requires privacy model rethink)
