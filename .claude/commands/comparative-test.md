Run blind vs axon-informed agent comparison.

Run: `python3 scripts/comparative_stress_test.py --axon-bin ./target/release/axon --duration 30`

Report:
- Scenario A (blind): avg CPU during stress, peak RAM, failure count
- Scenario B (axon-informed): avg CPU during stress, peak RAM, failure count
- Delta between scenarios (% improvement)
- Whether axon-informed agents performed measurably better
