#!/usr/bin/env bash
# Live E2E: alert-dispatch.json + AXON_CONFIG_DIR, real axon serve, wait for webhook POST.
# Optional: ALERT_E2E_WAIT=90 (seconds), AXON_E2E_DEBUG=1 for stderr from axon.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
BIN="${AXON_BIN:-$ROOT/target/release/axon}"
[[ -f "$BIN" ]] || cargo build --release -p axon -q
exec python3 "$ROOT/scripts/test_alert_webhooks_live.py" "$BIN"
