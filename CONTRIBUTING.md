# Contributing to Axon

Thanks for helping build Axon. The project goal is simple: give local AI
agents useful runtime awareness without sending machine data to a cloud service.

## Development Setup

Install Rust stable, then run:

```bash
cargo build
cargo test --workspace
cargo clippy --workspace -- -D warnings
cargo fmt --all -- --check
```

The CLI binary is the `axon` package:

```bash
cargo run -p axon -- diagnose
cargo run -p axon -- query hw_snapshot
cargo run -p axon -- query workload_advice
```

## Pull Request Checklist

- Keep changes scoped to one feature, fix, or documentation improvement.
- Add or update tests when behavior changes.
- Run format, clippy, and tests before opening a PR.
- Do not add telemetry, analytics, or outbound network calls.
- Do not write to stdout from the MCP server path; stdout is reserved for
  JSON-RPC.
- Include real user impact in the PR description when changing agent behavior.

## Good First Contributions

Useful areas for contributors:

- additional workload-advice scenarios
- better Windows/Linux hardware signal coverage
- agent-runtime detection for more local tools
- public engineering analysis writeups with reproducible test scripts
- documentation for integrating Axon with MCP clients

## Reporting Bugs

Please include:

- operating system and version
- Axon version or commit
- command or MCP client used
- expected behavior
- actual behavior
- relevant logs from stderr, never private machine data

## Privacy Rule

Axon is zero-cloud by design. Contributions must preserve that boundary.
Hardware and process data should remain local unless a user explicitly exports
it themselves.
