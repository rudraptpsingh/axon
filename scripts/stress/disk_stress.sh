#!/usr/bin/env bash
# Disk stress generator: fills a temp directory with large files.
# Writes PIDs and paths to files for cleanup.
set -euo pipefail

PID_FILE="${1:-/tmp/axon_stress_disk.pids}"
PATH_FILE="${2:-/tmp/axon_stress_disk.paths}"
TARGET_DIR="${3:-/tmp/axon_disk_stress}"

mkdir -p "$TARGET_DIR"
: > "$PID_FILE"
: > "$PATH_FILE"
echo "$TARGET_DIR" >> "$PATH_FILE"

# Write 4GB of data in 1GB chunks (enough to push most dev machines toward disk pressure)
for i in $(seq 1 4); do
    FILE="$TARGET_DIR/stress_$i.dat"
    dd if=/dev/zero of="$FILE" bs=1M count=1024 2>/dev/null &
    echo $! >> "$PID_FILE"
    echo "$FILE" >> "$PATH_FILE"
done

echo "[info] Disk stress started: writing 4x 1GB files to $TARGET_DIR (PIDs in $PID_FILE)"
wait 2>/dev/null || true
echo "[info] Disk stress files written"
