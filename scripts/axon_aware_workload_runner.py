#!/usr/bin/env python3
"""
Axon-Aware Workload Runner

Wraps stress workloads with Axon MCP tool queries for intelligent scheduling.
Each agent queries hw_snapshot/process_blame/session_health before, during,
and after its workload, logging every decision.

MCP protocol follows the proven pattern from mcp_exercise_all_tools.py:
  - JSON-RPC 2.0 over stdio
  - initialize + notifications/initialized handshake
  - Buffered response reading (accumulate until target ID)
  - Parse: result["result"]["content"][0]["text"] → json.loads()
"""
from __future__ import annotations

import json
import subprocess
import sys
import time
from typing import Any


# ---------------------------------------------------------------------------
# MCP client (same pattern as mcp_exercise_all_tools.py / perf_test_scenario.py)
# ---------------------------------------------------------------------------

def mcp_send(proc: subprocess.Popen[str], obj: dict[str, Any]) -> None:
    """Send a JSON-RPC message to the Axon MCP server."""
    assert proc.stdin
    proc.stdin.write(json.dumps(obj) + "\n")
    proc.stdin.flush()


def mcp_read_until_id(proc: subprocess.Popen[str], target_id: int, timeout_s: float = 30.0) -> dict[str, Any] | None:
    """Read newline-delimited JSON until we see a response with the target id."""
    assert proc.stdout
    deadline = time.time() + timeout_s
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
        if msg.get("id") == target_id and "result" in msg:
            return msg
    return None


def mcp_parse_tool_text(resp: dict[str, Any]) -> dict[str, Any] | None:
    """Extract the JSON payload from an MCP tool call response."""
    try:
        text = resp["result"]["content"][0]["text"]
        return json.loads(text)
    except (KeyError, IndexError, json.JSONDecodeError):
        return None


class AxonMCPClient:
    """Stateful MCP client for querying Axon tools."""

    def __init__(self, proc: subprocess.Popen[str]):
        self.proc = proc
        self._next_id = 10  # start after init IDs
        self._initialized = False

    def initialize(self) -> bool:
        """Perform MCP initialize handshake.  Must be called once before tool calls."""
        mcp_send(self.proc, {
            "jsonrpc": "2.0", "id": 0, "method": "initialize",
            "params": {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "axon-agent-demo", "version": "0.1.0"},
            },
        })
        mcp_send(self.proc, {"jsonrpc": "2.0", "method": "notifications/initialized"})
        resp = mcp_read_until_id(self.proc, 0, timeout_s=15.0)
        self._initialized = resp is not None
        return self._initialized

    def call_tool(self, tool_name: str, arguments: dict[str, Any] | None = None) -> dict[str, Any] | None:
        """Call an Axon MCP tool by name.  Returns parsed JSON data or None."""
        if not self._initialized:
            return None
        mid = self._next_id
        self._next_id += 1
        mcp_send(self.proc, {
            "jsonrpc": "2.0", "id": mid, "method": "tools/call",
            "params": {"name": tool_name, "arguments": arguments or {}},
        })
        resp = mcp_read_until_id(self.proc, mid, timeout_s=30.0)
        if resp is None:
            return None
        return mcp_parse_tool_text(resp)

    def hw_snapshot(self) -> dict[str, Any] | None:
        return self.call_tool("hw_snapshot")

    def process_blame(self) -> dict[str, Any] | None:
        return self.call_tool("process_blame")

    def session_health(self) -> dict[str, Any] | None:
        return self.call_tool("session_health")

    def system_profile(self) -> dict[str, Any] | None:
        return self.call_tool("system_profile")

    def hardware_trend(self, time_range: str = "last_1h", interval: str = "15m") -> dict[str, Any] | None:
        return self.call_tool("hardware_trend", {"time_range": time_range, "interval": interval})


# ---------------------------------------------------------------------------
# Decision engine
# ---------------------------------------------------------------------------

def assess_headroom(hw: dict[str, Any] | None) -> str:
    """Determine headroom level from hw_snapshot response."""
    if hw is None or not hw.get("ok"):
        return "unknown"
    data = hw.get("data", {})
    return data.get("headroom", "unknown")


def assess_impact(blame: dict[str, Any] | None) -> str:
    """Determine impact level from process_blame response."""
    if blame is None or not blame.get("ok"):
        return "unknown"
    data = blame.get("data", {})
    return data.get("impact_level", "unknown")


def should_proceed(headroom: str, impact: str) -> tuple[str, str]:
    """Decide whether an agent should proceed, defer, or hold.

    Returns (decision, reason).
    Headroom "limited" alone is OK — only defer when combined with active impact.
    This prevents agents from stalling on systems with naturally limited headroom.
    """
    if headroom == "insufficient" or impact in ("critical", "strained"):
        return "hold", f"System stressed (headroom={headroom}, impact={impact})"
    if headroom == "limited" and impact in ("degrading",):
        return "defer", f"System busy (headroom={headroom}, impact={impact})"
    return "proceed", f"System clear (headroom={headroom}, impact={impact})"


# ---------------------------------------------------------------------------
# Agent workload runner
# ---------------------------------------------------------------------------

class AgentWorkloadRunner:
    """Run a single agent's workload with Axon-informed decision making."""

    def __init__(self, agent_name: str, mcp: AxonMCPClient, query_interval: float = 30.0):
        self.agent_name = agent_name
        self.mcp = mcp
        self.query_interval = query_interval
        self.decisions: list[dict[str, Any]] = []

    def log_decision(self, query_type: str, result: Any, decision: str, reason: str) -> None:
        self.decisions.append({
            "timestamp": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
            "agent": self.agent_name,
            "query": query_type,
            "result_summary": _summarize(result),
            "decision": decision,
            "reason": reason,
        })
        print(f"  [{self.agent_name}] {decision}: {reason}", file=sys.stderr)

    def wait_for_headroom(self, timeout_s: float = 300.0, poll_s: float = 5.0) -> str:
        """Poll Axon until headroom is adequate or timeout.  Returns final decision."""
        deadline = time.time() + timeout_s
        while time.time() < deadline:
            hw = self.mcp.hw_snapshot()
            blame = self.mcp.process_blame()
            headroom = assess_headroom(hw)
            impact = assess_impact(blame)
            decision, reason = should_proceed(headroom, impact)

            self.log_decision("hw_snapshot+process_blame", {"headroom": headroom, "impact": impact}, decision, reason)

            if decision == "proceed":
                return decision
            time.sleep(poll_s)

        self.log_decision("timeout", {}, "proceed_forced", f"Waited {timeout_s}s, proceeding anyway")
        return "proceed_forced"

    def monitor_during(self, proc: subprocess.Popen[Any], max_duration: float) -> None:
        """Monitor the running workload, querying Axon every query_interval seconds."""
        start = time.time()
        last_query = start
        while proc.poll() is None:
            now = time.time()
            if now - start > max_duration:
                break
            if now - last_query >= self.query_interval:
                hw = self.mcp.hw_snapshot()
                blame = self.mcp.process_blame()
                headroom = assess_headroom(hw)
                impact = assess_impact(blame)
                self.log_decision(
                    "monitoring",
                    {"headroom": headroom, "impact": impact},
                    "continue",
                    f"Monitoring: headroom={headroom}, impact={impact}",
                )
                last_query = now
            time.sleep(1)

    def post_workload_assessment(self) -> None:
        """Query Axon after workload completes for recovery state."""
        hw = self.mcp.hw_snapshot()
        health = self.mcp.session_health()
        headroom = assess_headroom(hw)
        alert_count = 0
        if health and health.get("ok"):
            alert_count = health.get("data", {}).get("alert_count", 0)
        self.log_decision(
            "post_workload",
            {"headroom": headroom, "alert_count": alert_count},
            "assessment",
            f"Post-workload: headroom={headroom}, alerts={alert_count}",
        )


def _summarize(obj: Any) -> Any:
    """Create a compact summary of a result for logging."""
    if isinstance(obj, dict):
        return {k: v for k, v in obj.items() if k not in ("raw", "narrative")}
    return obj
