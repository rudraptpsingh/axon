#!/usr/bin/env bash
# CPU stress generator: spawns yes + dd processes to saturate all cores.
# Writes PIDs to a file for cleanup.
set -euo pipefail

PID_FILE="${1:-/tmp/axon_stress_cpu.pids}"
NCPU=$(nproc 2>/dev/null || sysctl -n hw.ncpu 2>/dev/null || echo 4)
# Spawn yes processes (ncpu * 2 to ensure saturation)
COUNT=$((NCPU * 2))

: > "$PID_FILE"

for _ in $(seq 1 "$COUNT"); do
    yes > /dev/null 2>&1 &
    echo $! >> "$PID_FILE"
done

# Spawn dd processes for additional IO + CPU pressure
for _ in $(seq 1 "$NCPU"); do
    dd if=/dev/urandom of=/dev/null bs=1M count=99999 2>/dev/null &
    echo $! >> "$PID_FILE"
done

echo "[info] CPU stress started: $COUNT yes + $NCPU dd processes (PIDs in $PID_FILE)"
