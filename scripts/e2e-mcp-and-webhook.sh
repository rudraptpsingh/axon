#!/usr/bin/env bash
# Live E2E: alert-dispatch.json with explicit mcp + webhook channels (not --cli).
# Webhook POST is asserted; MCP path is covered by crates/axon-core/tests/alert_integration.rs.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="${AXON_BIN:-$ROOT/target/release/axon}"
[[ -f "$BIN" ]] || cargo build --release -p axon -q
exec python3 "$ROOT/scripts/test_alert_webhooks_live.py" --mcp-and-webhook "$BIN"
