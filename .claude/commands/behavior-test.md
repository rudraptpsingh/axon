Run the 4-phase agent behavior test (~6 minutes).

Run: `python3 scripts/agent_behavior_test.py --phases all --axon-binary ./target/release/axon`

Report:
- Phase 1 (baseline): throughput and P95 latency
- Phase 2 (stress): degradation % vs baseline
- Phase 3 (axon-informed): improvement % vs blind stress
- Phase 4 (recovery): did metrics return to baseline?
- Overall verdict: did axon awareness improve agent performance?
