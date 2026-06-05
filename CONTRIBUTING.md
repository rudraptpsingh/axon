# Contributing to Axon

Axon is a local-first MCP server for hardware and agent-runtime awareness. The
project is open to contributions that improve reliability, platform coverage,
agent integrations, documentation, or test evidence while preserving the
privacy boundary.

## Development Setup

Install Rust stable, then run:

```bash
cargo build
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

The CLI binary is the `axon` package:

```bash
cargo run -p axon -- diagnose
cargo run -p axon -- query hw_snapshot
cargo run -p axon -- query workload_advice
```

## Contribution Guidelines

- Keep changes scoped to one feature, fix, or documentation improvement.
- Prefer existing crate boundaries and local patterns over new abstractions.
- Add or update tests when behavior changes.
- Include real user impact when changing agent-facing behavior.
- Keep the MCP server stdout path JSON-RPC only; use stderr for logs.
- Do not add telemetry, analytics, or automatic outbound network calls.

## Useful Areas

- additional workload-advice scenarios
- Windows, Linux, and GPU signal coverage
- agent-runtime detection for more local tools
- reproducible engineering analysis and benchmark writeups
- MCP client integration documentation
- focused tests for alerting, persistence, grouping, and narratives

## Pull Requests

Before opening a pull request, run the relevant checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

For platform-specific behavior, note which operating systems were tested. For
changes that touch MCP tool output, include a sample response or explain the
compatibility impact.

## Reporting Bugs

Use the bug report template and include:

- operating system and version
- Axon version or commit
- command or MCP client used
- expected behavior
- actual behavior
- reproduction steps
- relevant stderr logs

Do not include secrets, credentials, private process data, or machine logs that
you do not intend to share publicly.

## Security Reports

Do not open public issues for vulnerabilities. Follow
[SECURITY.md](SECURITY.md) instead.

## Privacy Rule

Axon is zero-cloud by design. Contributions must preserve that boundary.
Hardware and process data should remain local unless a user explicitly exports
or forwards it.
