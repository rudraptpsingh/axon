#!/usr/bin/env python3
"""
Drive axon serve over stdio: initialize, tools/list, then call each MCP tool.
Exits 0 only if every step returns ok JSON with expected shape.
Usage: mcp_exercise_all_tools.py /path/to/axon
"""
from __future__ import annotations

import json
import subprocess
import sys
from typing import Any


def send(proc: subprocess.Popen[str], obj: dict[str, Any]) -> None:
    assert proc.stdin
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def read_responses(proc: subprocess.Popen[str], until_id: int, timeout_s: float = 30.0) -> list[dict[str, Any]]:
    """Read newline-delimited JSON until we see a response with jsonrpc id == until_id."""
    import time

    buf: list[dict[str, Any]] = []
    deadline = time.time() + timeout_s
    assert proc.stdout
    while time.time() < deadline:
        line = proc.stdout.readline()
        if not line:
            break
        line = line.strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        buf.append(msg)
        if msg.get("id") == until_id and "result" in msg:
            return buf
    raise RuntimeError(f"timeout or EOF waiting for id={until_id}; got {buf[-3:]!r}")


def parse_mcp_tool_text(result: dict[str, Any]) -> dict[str, Any]:
    content = result["result"]["content"]
    assert len(content) >= 1
    text = content[0]["text"]
    return json.loads(text)


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: mcp_exercise_all_tools.py /path/to/axon", file=sys.stderr)
        return 2

    axon = sys.argv[1]
    proc = subprocess.Popen(
        [axon, "serve"],
        stdin=subprocess.PIPE,
        stdout=subprocess.PIPE,
        stderr=subprocess.DEVNULL,
        text=True,
        bufsize=1,
    )
    try:
        send(
            proc,
            {
                "jsonrpc": "2.0",
                "id": 0,
                "method": "initialize",
                "params": {
                    "protocolVersion": "2025-06-18",
                    "capabilities": {},
                    "clientInfo": {"name": "e2e-script", "version": "0.1.0"},
                },
            },
        )
        send(proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        read_responses(proc, 0)

        send(
            proc,
            {"jsonrpc": "2.0", "id": 1, "method": "tools/list", "params": {}},
        )
        list_msgs = read_responses(proc, 1)
        tools = list_msgs[-1]["result"]["tools"]
        names = {t["name"] for t in tools}
        expected = {
            "hw_snapshot",
            "process_blame",
            "battery_status",
            "system_profile",
            "hardware_trend",
            "session_health",
        }
        missing = expected - names
        if missing:
            print(f"[err] tools/list missing: {missing}", file=sys.stderr)
            return 1

        calls: list[tuple[int, str, dict[str, Any]]] = [
            (2, "hw_snapshot", {}),
            (3, "process_blame", {}),
            (4, "battery_status", {}),
            (5, "system_profile", {}),
            (6, "hardware_trend", {"time_range": "last_1h", "interval": "15m"}),
            (7, "session_health", {}),
        ]

        for mid, name, args in calls:
            send(
                proc,
                {
                    "jsonrpc": "2.0",
                    "id": mid,
                    "method": "tools/call",
                    "params": {"name": name, "arguments": args},
                },
            )
            msgs = read_responses(proc, mid, timeout_s=45.0)
            res = msgs[-1]
            if "error" in res:
                print(f"[err] {name}: {res['error']}", file=sys.stderr)
                return 1
            data = parse_mcp_tool_text(res)
            if name == "battery_status":
                if data.get("ok"):
                    assert "data" in data
                else:
                    print("[info] battery_status: ok=false (e.g. desktop or no pmset data)")
            else:
                if not data.get("ok"):
                    print(f"[err] {name}: expected ok true, got {data!r}", file=sys.stderr)
                    return 1
                assert "data" in data
            print(
                f"[ok] {name}: ok={data.get('ok')} "
                f"narrative_len={len(str(data.get('narrative', '')))}"
            )

        print("[ok] all MCP tools responded")
        return 0
    finally:
        proc.terminate()
        try:
            proc.wait(timeout=3)
        except subprocess.TimeoutExpired:
            proc.kill()


if __name__ == "__main__":
    raise SystemExit(main())
