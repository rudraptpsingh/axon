# mcp-station Roadmap

## The Vision
A single command that tells your AI dev session exactly what is breaking your Mac and what to do — without sending a single byte off-device.

---

## Phase 1: MVP (14-Day Sprint)

### Completed

- [x] **Core types & data model** — `HwSnapshot`, `ProcessBlame`, `BatteryStatus`, `SystemProfile`, `McpResponse<T>` envelope
- [x] **EWMA baseline tracker** — Per-process rolling average (α=0.2), 3+ sample warm-up before reporting deltas
- [x] **Impact engine** — Multi-signal anomaly scoring (0.4×RAM + 0.3×CPU + 0.3×swap), 4-tier mapping (Healthy/Degrading/Strained/Critical), human-readable messages
- [x] **Hardcoded fix table** — Process-specific fixes for Cursor, cargo, node, python, Docker, Ollama, VS Code
- [x] **Temperature reader** — SMC sensor detection via sysinfo components
- [x] **Collector loop** — 2-second tick, background sysinfo refresh, EWMA updates, battery via `pmset -g batt`
- [x] **MCP server** — 4 tools (`hw_snapshot`, `process_blame`, `battery_status`, `system_profile`) via rmcp `#[tool_router]`
- [x] **CLI** — `serve`, `diagnose`, `status`, `setup` commands
- [x] **Auto-setup** — Configures Claude Desktop and Cursor automatically on first run
- [x] **Agent integration** — `mcp-station setup claude-desktop | claude-cli | cursor`

### Remaining (Days 8–14)

- [ ] **Homebrew tap** — Create `homebrew-mcp-station` tap repo, write formula, test `brew install mcp-station`
- [ ] **GitHub release automation** — CI workflow to build universal macOS binary, create GitHub release with checksums
- [ ] **README** — Installation instructions, usage examples, architecture diagram
- [ ] **Demo GIF** — Screen recording showing `process_blame` catching a runaway process in Claude Desktop
- [ ] **Dogfood & polish** — Run as daily driver for 2 days, fix edge cases (zombie processes, sleep/wake, lid close)
- [ ] **Launch** — Show HN post, Twitter thread, Product Hunt listing

---

## Phase 2: Traction & Growth

Features cut from MVP to ship fast. Revisit after validating with real users.

| Feature | Description | Why Deferred |
|---|---|---|
| **GPU attribution** | Track Metal/GPU usage per process, detect GPU memory pressure | Requires IOKit integration, complex on Apple Silicon |
| **SSE streaming transport** | Real-time push updates instead of poll-on-demand | stdio works for all current MCP clients |
| **Cross-session memory** | SQLite persistence, track patterns across restarts | In-memory EWMA is sufficient for MVP |
| **Windsurf / Continue support** | Auto-setup for more AI agents | Add as users request; config format is identical |
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

**Key design decisions:**
- **Privacy-by-architecture** — No network calls, no telemetry, pure local
- **stdio transport** — Universal MCP compatibility (Claude Desktop, Cursor, Claude Code)
- **EWMA over Kalman** — Simpler, good enough for 2s sample rate
- **In-memory only** — No database, no persistence, restart = fresh baseline
- **Auto-setup on first run** — Zero friction for new users
