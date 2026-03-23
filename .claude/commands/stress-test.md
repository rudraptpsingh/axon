Run a quick CPU stress test and verify axon detection.

Steps:
1. `axon query hw_snapshot` — record baseline CPU, RAM, headroom
2. Start stress: `for i in 1 2 3 4; do yes > /dev/null & done`
3. Wait 15 seconds for axon to collect samples
4. `axon query hw_snapshot` — record stressed CPU, RAM, headroom
5. `axon query process_blame` — check if `yes` is the culprit
6. Kill stress: `kill $(jobs -p) 2>/dev/null; pkill -f "yes" 2>/dev/null`
7. Wait 10 seconds for recovery
8. `axon query hw_snapshot` — record recovery CPU, RAM, headroom

Report: baseline vs stress vs recovery comparison table. Did axon correctly identify the `yes` processes as the culprit? Did headroom transition appropriately? Did impact level match the actual stress?
