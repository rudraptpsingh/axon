#!/usr/bin/env python3
"""
Async Queue Task - simulates an agent processing items from a queue.

Behavior:
- Phase 1-2 (default): Async mode - non-blocking dequeue, queue can grow
- Phase 3 (adaptation): Switch to sync mode - blocking dequeue (1 item at a time)

Measures: throughput (items/sec), queue depth, latency percentiles, memory usage
"""

import asyncio
import json
import os
import sys
import time
from collections import deque
from datetime import datetime
from typing import Any


class AsyncQueueTask:
    """Simulates an agent processing items from an async queue."""

    def __init__(self, total_items: int = 1000, sync_mode: bool = False):
        self.total_items = total_items
        self.sync_mode = sync_mode
        self.queue: asyncio.Queue[int] = asyncio.Queue()
        self.processed_count = 0
        self.latencies: deque[float] = deque(maxlen=100)  # Track last 100 latencies
        self.start_time = None
        self.samples = []

    def set_sync_mode(self, enable: bool):
        """Switch between sync and async modes."""
        self.sync_mode = enable

    async def enqueue_items(self):
        """Producer: enqueue items rapidly."""
        for i in range(self.total_items):
            await self.queue.put(i)
            await asyncio.sleep(0.001)  # Minimal delay between enqueues

    async def dequeue_worker(self, worker_id: int):
        """Worker: dequeue and process items."""
        while True:
            try:
                if self.sync_mode:
                    # Sync mode: block until item available (slows down processing)
                    try:
                        item = self.queue.get_nowait()
                    except asyncio.QueueEmpty:
                        break
                else:
                    # Async mode: non-blocking dequeue
                    item = await asyncio.wait_for(self.queue.get(), timeout=0.1)

                # Simulate work
                work_start = time.time()
                await asyncio.sleep(0.002)
                work_latency = (time.time() - work_start) * 1000  # ms

                self.latencies.append(work_latency)
                self.processed_count += 1
                self.queue.task_done()

            except (asyncio.TimeoutError, asyncio.QueueEmpty):
                await asyncio.sleep(0.01)

    def get_memory_mb(self) -> float:
        """Get current process memory usage in MB."""
        try:
            pid = os.getpid()
            with open(f"/proc/{pid}/status") as f:
                for line in f:
                    if line.startswith("VmRSS:"):
                        rss_kb = int(line.split()[1])
                        return rss_kb / 1024
            return 0
        except Exception:
            return 0

    def collect_sample(self) -> dict[str, Any]:
        """Collect one sample of metrics."""
        elapsed = time.time() - self.start_time if self.start_time else 0
        throughput = self.processed_count / elapsed if elapsed > 0 else 0

        # Calculate latency percentiles
        if self.latencies:
            latency_list = sorted(self.latencies)
            p50 = latency_list[len(latency_list) // 2]
            p95 = latency_list[int(len(latency_list) * 0.95)]
            p99 = latency_list[int(len(latency_list) * 0.99)]
        else:
            p50 = p95 = p99 = 0

        sample = {
            "timestamp": datetime.now().isoformat(),
            "elapsed_sec": elapsed,
            "mode": "sync" if self.sync_mode else "async",
            "items_processed": self.processed_count,
            "queue_depth": self.queue.qsize(),
            "throughput_items_sec": round(throughput, 2),
            "latency_p50_ms": round(p50, 2),
            "latency_p95_ms": round(p95, 2),
            "latency_p99_ms": round(p99, 2),
            "memory_mb": round(self.get_memory_mb(), 2),
        }
        self.samples.append(sample)
        return sample

    async def run(self, duration_s: float, num_workers: int = 2, sample_interval: float = 2.0):
        """Run the queue processing task for specified duration."""
        self.start_time = time.time()
        deadline = self.start_time + duration_s

        # Start producer and workers
        producer_task = asyncio.create_task(self.enqueue_items())
        worker_tasks = [
            asyncio.create_task(self.dequeue_worker(i)) for i in range(num_workers)
        ]

        # Sampling task
        async def sample_loop():
            while time.time() < deadline:
                self.collect_sample()
                await asyncio.sleep(sample_interval)

        sampler_task = asyncio.create_task(sample_loop())

        # Wait until deadline
        while time.time() < deadline:
            await asyncio.sleep(0.1)

        # Stop everything
        producer_task.cancel()
        for task in worker_tasks:
            task.cancel()
        sampler_task.cancel()

        # Wait for cancellations
        await asyncio.gather(producer_task, *worker_tasks, sampler_task, return_exceptions=True)

        # Final sample
        self.collect_sample()

    def save_samples(self, output_file: str):
        """Save samples to JSON file."""
        os.makedirs(os.path.dirname(output_file) or ".", exist_ok=True)
        with open(output_file, "w") as f:
            json.dump(self.samples, f, indent=2)
        print(f"[ok] async_queue_task: {len(self.samples)} samples → {output_file}", file=sys.stderr)


async def main():
    import argparse

    ap = argparse.ArgumentParser(description="Async queue processing task")
    ap.add_argument("output_file", help="JSON file to write task samples to")
    ap.add_argument("--duration", type=float, default=60, help="Duration in seconds")
    ap.add_argument("--total-items", type=int, default=1000, help="Total items to process")
    ap.add_argument("--workers", type=int, default=2, help="Number of worker tasks")
    ap.add_argument("--sync-mode", action="store_true", help="Start in sync mode")
    args = ap.parse_args()

    task = AsyncQueueTask(total_items=args.total_items, sync_mode=args.sync_mode)
    await task.run(duration_s=args.duration, num_workers=args.workers)
    task.save_samples(args.output_file)


if __name__ == "__main__":
    asyncio.run(main())
