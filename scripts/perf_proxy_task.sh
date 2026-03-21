#!/usr/bin/env bash
# Proxy benchmark task: a small C compilation sensitive to CPU/memory pressure.
# Returns wall-clock time in seconds (float) on stdout.
set -euo pipefail

SRC="/tmp/axon_bench_task.c"

cat > "$SRC" << 'CSRC'
#include <stdio.h>
int main(void) {
    volatile long s = 0;
    for (long i = 0; i < 200000000L; i++) s += i;
    printf("%ld\n", s);
    return 0;
}
CSRC

START=$(date +%s%N)
cc -O2 -o /tmp/axon_bench_task "$SRC" 2>/dev/null
/tmp/axon_bench_task > /dev/null
END=$(date +%s%N)

ELAPSED_NS=$((END - START))
# Print seconds with 2 decimal places using awk
awk "BEGIN { printf \"%.2f\n\", $ELAPSED_NS / 1000000000.0 }"
