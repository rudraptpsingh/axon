#!/usr/bin/env bash
# Calls an axon MCP tool and pretty-prints the result.
# Usage: ./mcp-call.sh process_blame

set -euo pipefail
TOOL="${1:?Usage: $0 <tool_name>}"

(
  echo '{"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo","version":"0.1.0"}},"jsonrpc":"2.0","id":0}'
  echo '{"method":"notifications/initialized","jsonrpc":"2.0"}'
  sleep 4
  echo "{\"method\":\"tools/call\",\"params\":{\"name\":\"${TOOL}\",\"arguments\":{}},\"jsonrpc\":\"2.0\",\"id\":1}"
  sleep 1
) | axon serve 2>/dev/null \
  | tail -1 \
  | python3 -c "
import sys, json
raw = json.load(sys.stdin)
text = raw['result']['content'][0]['text']
parsed = json.loads(text)
print(json.dumps(parsed, indent=2))
"
