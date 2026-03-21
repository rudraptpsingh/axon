#!/usr/bin/env bash
# Live E2E: same as config-file mode but uses --config-dir + --alert-webhook (no JSON file).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="${AXON_BIN:-$ROOT/target/release/axon}"
[[ -f "$BIN" ]] || cargo build --release -p axon -q
exec python3 "$ROOT/scripts/test_alert_webhooks_live.py" --cli "$BIN"
