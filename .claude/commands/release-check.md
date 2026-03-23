Pre-release validation checklist. Run all steps in order, stop at first failure.

1. `cargo build --release` — must compile cleanly
2. `cargo test --workspace` — all tests pass
3. `python3 scripts/eval_cpu_stress.py ./target/release/axon` — all checks pass
4. `python3 scripts/mcp_exercise_all_tools.py ./target/release/axon` — all 7 tools respond with ok=true
5. `bash scripts/e2e-real-scenario.sh` — all steps pass
6. Verify `axon --version` matches version in crates/axon-cli/Cargo.toml

Report: [ok] or [FAIL] for each step. If all pass, say "[ok] Ready to release."
