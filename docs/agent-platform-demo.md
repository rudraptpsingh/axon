# Axon Agent Platform Demo

## One-liner

Axon is a local runtime governor for AI app-building agents: it helps an
agent worker decide when to proceed, reduce parallelism, reuse tools, or pause
before wasting credits on unhealthy build/debug/deploy loops.

## Why Agent Platforms Should Care

Modern agent platforms turn natural-language prompts into app changes,
automation, tests, browser runs, deployments, and code reviews. That requires
agents to plan, code, run tools, test, debug, and often spawn multiple MCP
servers or tool workers. The expensive failure mode is not just "high CPU"; it
is a worker repeatedly spawning local processes, running parallel retries, or
continuing a doomed debug loop while credits and user patience burn.

Axon makes that worker state visible and actionable before the agent spends
more time or credits.

## Live Demo Command

```bash
python3 scripts/demo_agent_platform_live.py /Users/rp/github/axon/target/debug/axon
```

## What The Demo Proves

1. **Without Axon policy**, an app-builder worker can spawn additional
   tool/MCP-like processes and increase runtime pressure.
2. **With Axon policy**, the agent avoids new risky tool fan-out.
3. Axon still allows useful work by capping parallelism rather than blocking
   everything.
4. Axon detects desktop/runtime CPU pressure that would make the builder feel
   slow or stuck.
5. Temporary proof processes are cleaned up and the runtime returns to baseline.

## Example Output

```text
[without Axon policy] spawned 4 extra tool workers; MCP 45->49; playwright group 3->7
[with Axon policy] requested 4 new tool workers; spawned 0; avoided 4
[parallel build/debug loop] without Axon concurrency=4; with Axon concurrency=1; capped 4->1
[runtime UX guard] renderer CPU 6%->49%; high_cpu_ui=1
PITCH TAKEAWAY
Axon lets an app-building agent continue safe work while avoiding wasted credits.
```

## Pitch Narrative

> Agent platforms already know how to plan and execute work. Axon is the local
> runtime guardrail that tells those agents when the machine is healthy enough
> to spend more credits. In this demo, the same agent behavior without Axon
> spawns four extra tool workers. With Axon, it spawns zero, caps parallel work,
> and still allows safe local progress.

## Integration Sketch

Run Axon as a sidecar in each build VM or sandbox:

```text
User prompt
  -> planning agent
  -> Axon preflight
  -> build/test/deploy worker
  -> Axon runtime guard
  -> user-visible outcome: proceed / reduce / reuse / pause
```

Agent policy:

```text
if Axon says "defer":
  run only lightweight inspection and ask user before spending more credits
if Axon says "safe_parallelism=1":
  cap build/test/debug retries to one worker
if Axon reports duplicate tool stacks:
  reuse existing tools or restart the sandbox before spawning more
if Axon reports runtime UI/tool pressure:
  pause desktop-heavy/browser-heavy workflows
```

## Metrics To Show In A Partner Call

- Extra tool workers avoided
- Parallelism cap applied
- Stale worker/tool count before and after cleanup
- Failed/retried build loops avoided
- Time or credits saved by not starting doomed work
- Proof that safe work still continues
