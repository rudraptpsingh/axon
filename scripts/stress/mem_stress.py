#!/usr/bin/env python3
"""Memory stress generator: allocates chunks of memory to push RAM usage up.
Writes PID to a file for cleanup. Holds allocations until killed.
"""
import os
import sys
import signal
import time

CHUNK_MB = 128
MAX_CHUNKS = 96  # up to ~12GB
PID_FILE = sys.argv[1] if len(sys.argv) > 1 else "/tmp/axon_stress_mem.pids"

# Write our PID for cleanup
with open(PID_FILE, "w") as f:
    f.write(str(os.getpid()) + "\n")

allocations = []
print(f"[info] Memory stress started (PID {os.getpid()}), allocating {CHUNK_MB}MB chunks...")

def cleanup(sig, frame):
    print(f"\n[info] Memory stress stopped, freeing {len(allocations)} chunks")
    allocations.clear()
    sys.exit(0)

signal.signal(signal.SIGTERM, cleanup)
signal.signal(signal.SIGINT, cleanup)

for i in range(MAX_CHUNKS):
    try:
        # Allocate and touch memory to ensure it is resident
        chunk = bytearray(CHUNK_MB * 1024 * 1024)
        for offset in range(0, len(chunk), 4096):
            chunk[offset] = 0xFF
        allocations.append(chunk)
        total_mb = (i + 1) * CHUNK_MB
        print(f"  allocated {total_mb}MB ({i + 1}/{MAX_CHUNKS} chunks)")
        time.sleep(0.5)
    except MemoryError:
        print(f"[warn] MemoryError after {len(allocations)} chunks ({len(allocations) * CHUNK_MB}MB)")
        break

print(f"[info] Memory stress holding {len(allocations) * CHUNK_MB}MB. Waiting to be killed...")
# Hold allocations until killed
while True:
    time.sleep(60)
