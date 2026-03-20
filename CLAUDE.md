# CLAUDE.md -- axon

## Project Overview

axon is a zero-cloud, privacy-first MCP (Model Context Protocol) server that gives AI coding agents real-time local hardware awareness on macOS. It tells developers what is slowing their Mac and how to fix it -- without sending a single byte off-device.

## Architecture

```
crates/
  axon-core/     # Data types, EWMA baseline tracker, impact engine, process grouping, collector loop
  axon-server/   # MCP server (4 tools via rmcp #[tool_router])
  axon-cli/      # Binary: serve | diagnose | status | setup
```

- **axon-core** is a library crate. All data types live in `types.rs`. The collector loop in `collector.rs` runs every 2 seconds, refreshing sysinfo and updating per-process EWMA baselines. Process grouping in `grouping.rs` aggregates child processes by app name (e.g., Chrome helpers → "Google Chrome").
- **axon-server** exposes 4 MCP tools over stdio: `hw_snapshot`, `process_blame`, `battery_status`, `system_profile`. Uses rmcp 1.x with `#[tool_router]` and `#[tool_handler]` macros.
- **axon-cli** is the binary entry point (package name `axon`). It auto-configures Claude Desktop, Cursor, and VS Code on first run.

## Key Technical Details

- **rmcp quirk**: `serve()` returns a `RunningService` handle. You MUST call `.waiting().await` on it or the server exits immediately after initialization. This was a hard-won lesson.
- **stdio contract**: stdout is reserved exclusively for MCP JSON-RPC. All logging goes to stderr via `tracing`. Never `println!` from the server path.
- **Claude Desktop PATH**: Claude Desktop's subprocess PATH is limited to system directories. Always use absolute binary paths in `claude_desktop_config.json`.
- **sysinfo 0.33**: `component.temperature()` returns `Option<f32>`, not `f32`.
- **EWMA**: alpha=0.2, requires 3+ samples before reporting deltas. Persistence filter requires 3+ consecutive anomalous samples before escalating impact level.
- **No network calls**: This is a core design constraint. Never add telemetry, analytics, or any outbound network activity.

## Build & Test

```bash
cargo build                                # Debug build
cargo test --workspace                     # Unit tests (25 tests)
cargo test --workspace -- --ignored        # Integration tests (7 tests, ~10s)
cargo install --path crates/axon-cli       # Install to ~/.cargo/bin
axon diagnose                              # Quick smoke test
```

## Code Conventions

- **No emojis** in output, logs, or documentation. Use text-based indicators: `[ok]`, `[warn]`, `[err]`, `[info]`.
- **Minimal dependencies**: Do not add crates without justification. Prefer std where possible.
- **Error handling**: Use `anyhow::Result` for CLI/application code. MCP tool handlers return serialized JSON strings, never panic.
- **Naming**: snake_case for functions/variables, PascalCase for types. Tool names use snake_case to match MCP convention.
- **Tests**: Unit tests go in the same file (`#[cfg(test)] mod tests`). Integration tests go in `tests/` directories.
- **Commits**: Imperative mood, concise first line, body explains "why" not "what".

## MCP Tool Signatures

All 4 tools take `EmptyParams` (no arguments) and return a JSON string wrapped in `McpResponse<T>`:
```json
{"ok": true, "ts": "...", "data": {...}, "narrative": "human-readable summary"}
```

## Agent Setup Targets

The CLI supports: `claude-desktop`, `claude-code`, `cursor`, `vscode`. Each writes to the agent's config file using the appropriate JSON structure. Auto-setup runs silently on every invocation if not already configured.

## What NOT To Do

- Do not add network calls or telemetry of any kind
- Do not write to stdout from the server path (breaks MCP JSON-RPC)
- Do not use `std::sync::Mutex` in async code paths without careful consideration (current usage is safe because locks are held briefly)
- Do not add GPU monitoring yet (Phase 3)
- Do not add persistence/SQLite yet (Phase 2)
- Do not add team/fleet APIs yet (Phase 3)
