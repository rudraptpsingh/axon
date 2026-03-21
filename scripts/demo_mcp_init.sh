#!/usr/bin/env bash
# Writes MCP initialize + initialized messages to /tmp/axon_init.txt for demo tapes
printf '{"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"demo","version":"0.1.0"}},"jsonrpc":"2.0","id":0}\n{"method":"notifications/initialized","jsonrpc":"2.0"}\n' > /tmp/axon_init.txt
