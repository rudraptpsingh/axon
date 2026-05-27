# Axon Value Scorecard

## Current Agent Runtime

- Agent processes: `68`
- MCP/tool servers: `39`
- Stale MCP/tool servers: `33`
- MCP/tool RAM: `77 MB`
- Agent CPU: `30%`
- Agent RAM: `507 MB`
- Workload recommendation: `defer`
- Safe parallelism: `1`

## Value Created

- Risky tool spawns avoided: `4`
- Parallel workers avoided: `3`
- Estimated credits saved: `4.75`
- Estimated time saved: `4.5 min`
- Estimated developer cost avoided: `$7.5`

## Why Axon Acted

- duplicate MCP/tool groups: computer-use-mcp x8, figbridge-mcp x16, node x2, node_repl x7, oversight-mcp x4, playwright-mcp x2
- 33 stale MCP/tool servers
- workload advice=defer

## Live Proof

- No-policy MCP count: `39 -> 43`
- Cleanup MCP count: `39`
- Playwright/tool group: `2 -> 6`

## Workflow Impact

- **Long agent coding session**: Prevents slowdowns where every new prompt, tool call, or file edit competes with stale tool servers from old sessions.
  Action: Save work, restart the agent host, then reopen only the active workspace before launching more subagents or MCP-heavy tools.
- **Multi-session agent workspace**: Reduces failed or flaky runs caused by old sessions consuming file handles, memory, terminal slots, and MCP connections.
  Action: Cleanly restart the agent app after saving work, then rerun Axon to confirm stale runtime count dropped.

## User-Facing Summary

Axon avoided 4 risky tool spawns, reduced parallelism by 3 workers, and estimated 4.5 minutes of wasted work avoided.
