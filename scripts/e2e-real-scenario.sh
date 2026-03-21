#!/usr/bin/env bash
# End-to-end: build axon and exercise every CLI entrypoint + all MCP tools.
# No network except localhost MCP stdio to the binary.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="${AXON_BIN:-$ROOT/target/release/axon}"
PY="${PYTHON:-python3}"

cd "$ROOT"

echo "== [1/7] build release binary =="
cargo build --release -p axon -q

echo "== [2/7] --version / --help =="
"$BIN" -V
"$BIN" --help | head -n 5

echo "== [3/7] status (JSON) =="
OUT=$("$BIN" status)
echo "$OUT" | "$PY" -c 'import json,sys; json.load(sys.stdin); print("[ok] status parses as JSON")'

echo "== [4/7] diagnose (4s+ collection) =="
"$BIN" diagnose | head -n 20

echo "== [5/7] setup cursor (idempotent) =="
"$BIN" setup cursor || true

echo "== [6/7] setup invalid target (expect failure) =="
set +e
"$BIN" setup not-a-real-target 2>/dev/null
EC=$?
set -e
if [[ "$EC" -eq 0 ]]; then
  echo "[err] setup garbage should have failed"
  exit 1
fi
echo "[ok] invalid setup exited non-zero ($EC)"

echo "== [7/7] MCP: initialize, tools/list, all 5 tools =="
"$PY" "$ROOT/scripts/mcp_exercise_all_tools.py" "$BIN"

echo ""
echo "[ok] e2e-real-scenario: all steps passed"
