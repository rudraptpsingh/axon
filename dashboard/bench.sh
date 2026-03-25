#!/bin/bash
# Axon Dashboard - Image Processing Benchmark
# Usage: bench.sh <mode> <batch_size> <count>
set -e
MODE="${1:-setup}"
BATCH="${2:-50}"
COUNT="${3:-50}"
DIR="/tmp/axon-bench-images"

if [ "$MODE" = "setup" ]; then
  rm -rf "$DIR"
  mkdir -p "$DIR/src" "$DIR/out"
  # Generate test images fast (3000x2000, moderate noise for CPU-heavy resize)
  python3 -c "
from PIL import Image
import random, numpy as np
for i in range($COUNT):
    arr = np.random.randint(0, 256, (2000, 3000, 3), dtype=np.uint8)
    img = Image.fromarray(arr)
    img.save(f'$DIR/src/img_{i:03d}.jpg', quality=92)
" 2>&1 | tail -1
  echo '{"status":"ready","count":'$COUNT'}'
  exit 0
fi

if [ "$MODE" = "process" ]; then
  START=$(python3 -c "import time; print(time.time())")
  PROCESSED=0
  FAILED=0
  FILES=($(ls "$DIR/src/"*.jpg 2>/dev/null | head -$COUNT))
  TOTAL=${#FILES[@]}

  for ((i=0; i<${#FILES[@]}; i+=BATCH)); do
    BATCH_FILES=("${FILES[@]:i:BATCH}")
    PIDS=()
    for f in "${BATCH_FILES[@]}"; do
      base=$(basename "$f")
      (
        # Heavy processing: resize + gaussian blur + unsharp mask + strip metadata
        magick "$f" -resize 1200x900 -gaussian-blur 0x2 -unsharp 2x1+1+0.05 -strip -quality 85 "$DIR/out/$base" 2>/dev/null
      ) &
      PIDS+=($!)
    done
    for pid in "${PIDS[@]}"; do
      if wait $pid 2>/dev/null; then
        PROCESSED=$((PROCESSED + 1))
      else
        FAILED=$((FAILED + 1))
      fi
    done
  done

  END=$(python3 -c "import time; print(time.time())")
  ELAPSED=$(python3 -c "print(f'{$END - $START:.1f}')")
  echo "{\"status\":\"done\",\"processed\":$PROCESSED,\"failed\":$FAILED,\"total\":$TOTAL,\"elapsed_s\":$ELAPSED,\"batch_size\":$BATCH}"
  exit 0
fi
