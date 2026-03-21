#!/usr/bin/env bash
# Cleanup all stress generators. Kills PIDs and removes temp files.
set -uo pipefail

kill_from_file() {
    local pf="$1"
    if [ -f "$pf" ]; then
        while IFS= read -r pid; do
            [ -n "$pid" ] && kill "$pid" 2>/dev/null || true
        done < "$pf"
        rm -f "$pf"
    fi
}

kill_from_file /tmp/axon_stress_cpu.pids
kill_from_file /tmp/axon_stress_mem.pids
kill_from_file /tmp/axon_stress_disk.pids

# Remove disk stress files
if [ -f /tmp/axon_stress_disk.paths ]; then
    while IFS= read -r p; do
        [ -n "$p" ] && rm -rf "$p" 2>/dev/null || true
    done < /tmp/axon_stress_disk.paths
    rm -f /tmp/axon_stress_disk.paths
fi

echo "[info] All stress generators cleaned up"
