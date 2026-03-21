#!/usr/bin/env bash
# Proxy benchmark task: CPU+memory-bandwidth work sensitive to system pressure.
# Returns wall-clock time in seconds (float) on stdout.
#
# Uses a random-access memory sweep (8MB working set) that thrashes the L2/L3
# caches and is very sensitive to CPU contention — contending `yes` processes
# force context switches mid-cache-line, degrading throughput measurably.
set -euo pipefail

SRC="/tmp/axon_bench_task.c"
BIN="/tmp/axon_bench_task"

# Only recompile when source changes (avoids measuring compile time)
NEED_COMPILE=0
if [ ! -f "$BIN" ]; then
    NEED_COMPILE=1
fi

cat > "${SRC}.new" << 'CSRC'
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

// 32MB working set — exceeds L2, thrashes L3 under contention
#define ARRAY_SIZE (32 * 1024 * 1024 / sizeof(int))
#define ITERATIONS 40

int main(void) {
    int *arr = (int *)malloc(ARRAY_SIZE * sizeof(int));
    if (!arr) return 1;

    // Initialize with pseudo-random values
    for (size_t i = 0; i < ARRAY_SIZE; i++)
        arr[i] = (int)(i * 2654435761u);

    volatile long checksum = 0;

    // Repeated random-stride sweeps: stride ensures cache-line misses
    for (int iter = 0; iter < ITERATIONS; iter++) {
        unsigned int idx = (unsigned int)(iter * 7 + 1);
        for (size_t step = 0; step < ARRAY_SIZE; step++) {
            idx = (idx * 2654435761u) % ARRAY_SIZE;
            arr[idx] += iter;
            checksum += arr[idx];
        }
    }

    printf("%ld\n", (long)checksum);
    free(arr);
    return 0;
}
CSRC

# Recompile only if source changed
if [ "$NEED_COMPILE" -eq 1 ] || ! cmp -s "${SRC}.new" "$SRC" 2>/dev/null; then
    mv "${SRC}.new" "$SRC"
    cc -O1 -o "$BIN" "$SRC" 2>/dev/null
else
    rm -f "${SRC}.new"
fi

START=$(date +%s%N)
"$BIN" > /dev/null
END=$(date +%s%N)

ELAPSED_NS=$((END - START))
awk "BEGIN { printf \"%.2f\n\", $ELAPSED_NS / 1000000000.0 }"
