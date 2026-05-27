# With and Without Axon: App Development A/B

## Scenario

The same small app-development task was run in independent temporary
workspaces. The blind run spawned all requested helper processes and ran checks
in parallel. The Axon-informed run queried workload advice first, avoided risky
tool fan-out, and capped concurrency.

## Results

| Metric | Blind agent | Axon-informed agent |
| --- | ---: | ---: |
| App tests passed | true | true |
| Tool helpers spawned | 4 | 0 |
| Max check concurrency | 4 | 1 |
| Subprocesses started | 8 | 4 |
| App output hash | `ad1ca7d958b3d975` | `ad1ca7d958b3d975` |
| App runtime benchmark ms | 95.09 | 52.78 |

## Interpretation

Axon did not make the agent "do less work" in a vague way. It changed the
execution policy before unnecessary local workers were spawned:

- avoided 4 extra tool helpers
- reduced max concurrency from 4 to 1
- preserved the same app output hash
- kept tests passing
- made the tradeoff explicit when the machine was already pressured

The Axon-informed path intentionally took longer wall-clock time in the
pressured run because it chose safer execution over blind fan-out. That is the
point of the policy: spend fewer local resources when the machine is already in
a state where more parallel work is likely to create failures, noise, or false
debugging paths.

## No-Degradation Fast Path

A separate lightweight edit and smoke-check path showed that Axon does not have
to slow useful validation work. Useful validation time stayed effectively the
same while unnecessary helper setup was avoided.

| Metric | Blind agent | Axon-informed agent |
| --- | ---: | ---: |
| Smoke checks passed | true | true |
| Useful validation seconds | 0.73 | 0.73 |
| Tool helpers spawned | 4 | 0 |

## Why This Matters

For agent IDEs, app builders, local runners, and hardware vendors, the value is
not a prettier system monitor. The value is an execution primitive:

```text
before expensive local work:
  ask Axon for host and agent-runtime state
  choose run, degrade, defer, or cleanup
  expose the reason to the user
```

That makes agentic workflows easier to trust because they can explain when the
machine, not the code, is the bottleneck.
