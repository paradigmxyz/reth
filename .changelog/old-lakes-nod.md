---
reth-engine-primitives: minor
reth-engine-tree: minor
reth-rpc-api: minor
reth-rpc-engine-api: patch
reth-bench: minor
reth-bb: patch
---

Added persistence backpressure tracking to `NewPayloadTimings`: `persistence_wait` is now a non-optional `Duration` that includes both time spent queued due to persistence backpressure and the explicit wait for in-flight persistence. Removed the `BENCH_RETH_NEW_PAYLOAD` toggle (always enabled), fixed `wait_time` to act as a minimum interval rather than a fixed sleep, and added warmup/wait-time metadata to benchmark summary output.
