# mcp-station Roadmap

## The Vision

A single command that tells your AI dev session exactly what is breaking your Mac and what to do -- without sending a single byte off-device.

---

## Phase 1: MVP (14-Day Sprint)

### Completed

- [x] Core types and data model (`HwSnapshot`, `ProcessBlame`, `BatteryStatus`, `SystemProfile`, `McpResponse<T>`)
- [x] EWMA baseline tracker (per-process rolling average, alpha=0.2, 3+ sample warm-up)
- [x] Impact engine (multi-signal scoring, 4-tier mapping, human-readable messages)
- [x] Hardcoded fix table (Cursor, cargo, node, python, Docker, Ollama, VS Code)
- [x] Temperature reader (SMC sensor detection via sysinfo)
- [x] Collector loop (2-second tick, background sysinfo refresh, battery via `pmset -g batt`)
- [x] MCP server (4 tools via rmcp `#[tool_router]`)
- [x] CLI (`serve`, `diagnose`, `status`, `setup`)
- [x] Auto-setup (Claude Desktop, Cursor, VS Code on first run)
- [x] Agent integration (`setup claude-desktop | claude-code | cursor | vscode`)
- [x] README, LICENSE, CLAUDE.md
- [x] GitHub Actions CI (check, test, clippy, fmt)
- [x] GitHub Actions release workflow (universal macOS binary, checksums)

### Remaining

- [ ] **Homebrew tap** -- Create `homebrew-mcp-station` tap repo, write formula, test `brew install mcp-station`
- [ ] **Demo recording** -- Terminal recording showing `process_blame` catching a runaway process
- [ ] **Dogfood and polish** -- Run as daily driver, fix edge cases (zombie processes, sleep/wake, lid close)
- [ ] **Launch** -- Show HN post, Twitter thread

---

## Phase 2: Traction & Growth

Features cut from MVP to ship fast. Revisit after validating with real users.

| Feature | Description | Why Deferred |
|---|---|---|
| **GPU attribution** | Track Metal/GPU usage per process, detect GPU memory pressure | Requires IOKit integration, complex on Apple Silicon |
| **SSE streaming transport** | Real-time push updates instead of poll-on-demand | stdio works for all current MCP clients |
| **Cross-session memory** | SQLite persistence, track patterns across restarts | In-memory EWMA is sufficient for MVP |
| **Windsurf support** | Auto-setup for Windsurf | Add when users request; config format is identical |
| **Linux support** | Extend beyond macOS | sysinfo abstracts most of it, but temp/battery need platform work |
| **Predictive alerts** | "Your RAM will hit critical in ~10 minutes" based on trend | Needs more historical data than EWMA provides |
| **Process grouping** | Group child processes (e.g., all Chrome helpers) into parent blame | Improves accuracy for multi-process apps |

---

## Phase 3: Monetization

Only after proving value with free tier.

| Feature | Description |
|---|---|
| **Pro tier** | Advanced analytics, historical trends, team dashboards |
| **Lemon Squeezy integration** | License key validation, payment flow |
| **Team insights** | Aggregate hardware profiles across a dev team |
| **CI/CD integration** | "Your build agent is thermal throttling" alerts in GitHub Actions |

---

## Architecture

```
mcp-station/
├── crates/
│   ├── mcp-station-core/        # Data types, EWMA, impact engine, collector
│   ├── mcp-station-server/      # MCP server (rmcp #[tool_router])
│   └── mcp-station-cli/         # Binary: serve | diagnose | status | setup
```

Key design decisions:
- **Privacy by architecture** -- no network calls, no telemetry, pure local
- **stdio transport** -- universal MCP compatibility (Claude Desktop, Cursor, Claude Code, VS Code)
- **EWMA over Kalman** -- simpler, good enough for 2-second sample rate
- **In-memory only** -- no database, no persistence, restart = fresh baseline
- **Auto-setup on first run** -- zero friction for new users
