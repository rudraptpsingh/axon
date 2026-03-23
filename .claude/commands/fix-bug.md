Investigate and fix a bug in axon's signal quality.

Follow this protocol:
1. Identify which signal is wrong (from the last /eval, /diagnose, or /stress-test output the user provides)
2. Read the relevant source files:
   - Types and data models: crates/axon-core/src/types.rs
   - Impact scoring: crates/axon-core/src/impact.rs
   - Collector loop: crates/axon-core/src/collector.rs
   - Alerts: crates/axon-core/src/alerts.rs
   - Thresholds: crates/axon-core/src/thresholds.rs
   - MCP server tools: crates/axon-server/src/lib.rs
   - CLI entry: crates/axon-cli/src/main.rs
3. Make the minimal fix — change the least code necessary
4. Add a unit test if the fix changes thresholds or scoring logic
5. Run `cargo test --workspace` — all must pass
6. Rebuild: `cargo build --release`
7. Re-run the scenario that exposed the bug and verify the fix
8. Report: what was wrong, root cause, what changed, before vs after
