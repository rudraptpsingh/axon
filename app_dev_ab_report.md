# App Development A/B: Without Axon vs With Axon

## Scenario

Both agents build and test the same small web app in independent temp workspaces.
The blind agent spawns all requested tool helpers and runs all checks in parallel.
The Axon-informed agent asks Axon first, avoids risky tool fan-out, and caps parallelism.

## Results

| Metric | Blind agent | Axon-informed agent |
| --- | ---: | ---: |
| App tests passed | True | True |
| Elapsed seconds | 5.82 | 14.02 |
| Tool helpers spawned | 4 | 0 |
| Max check concurrency | 4 | 1 |
| Subprocesses started | 8 | 4 |
| App output hash | `ad1ca7d958b3d975` | `ad1ca7d958b3d975` |
| App runtime benchmark ms | 95.09 | 52.78 |

## Value

- Tool helpers avoided: `4`
- Subprocesses avoided: `4`
- Parallelism reduction: `3`
- Elapsed delta with Axon: `8.199s`
- Estimated credits saved: `4.75`
- Estimated risk minutes avoided: `4.0`
- App output identical: `True`
- App benchmark delta with Axon: `-42.31ms`
- Tradeoff: Axon intentionally ran slower on this pressured machine to avoid extra tool fan-out and reduce concurrency.
- Interpretation: Axon preserved app-development success while reducing risky helper fan-out and parallelism.

## No-Degradation Fast Path

This separate lightweight app edit/smoke-check path shows Axon does not have to slow useful work.

| Metric | Blind agent | Axon-informed agent |
| --- | ---: | ---: |
| Smoke checks passed | True | True |
| Total elapsed seconds | 1.21 | 5.10 |
| Useful validation seconds | 0.73 | 0.73 |
| Tool helpers spawned | 4 | 0 |

- Fast-path elapsed delta with Axon: `3.884s`
- Useful validation delta with Axon: `0.006s`
- Fast-path interpretation: Axon avoided unnecessary tool setup without degrading useful app validation performance; the remaining overhead is preflight/decision time.

## Axon Decision Context

- Recommendation: `defer`
- Safe parallelism: `1`
- MCP before/after: `39` -> `39`
- Stale MCP before: `33`
- Workflow impacts: `Long agent coding session, Interactive IDE or desktop-agent work, Multi-session agent workspace, Build, test, Docker, and browser automation preflight`

