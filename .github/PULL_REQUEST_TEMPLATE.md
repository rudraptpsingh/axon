## Summary

Describe the change and why it belongs in Axon.

## User Impact

What developer, local-agent, or runtime-awareness problem does this improve?

## Implementation Notes

Call out any changes to MCP tool output, persistence format, platform-specific
logic, alert behavior, or setup/configuration behavior.

## Testing

- [ ] `cargo fmt --all -- --check`
- [ ] `cargo clippy --workspace -- -D warnings`
- [ ] `cargo test --workspace`

Add platform-specific or manual checks here when relevant:

```text

```

## Privacy Check

- [ ] No telemetry, analytics, or automatic outbound network calls added
- [ ] Local hardware/process data stays local unless explicitly configured by the user
- [ ] MCP server stdout remains JSON-RPC only
