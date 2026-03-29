---
reth-chain-state: minor
reth-engine-primitives: minor
reth-engine-tree: minor
reth-node-core: minor
reth-node-events: minor
reth: patch
---

Added configurable slow block logging (`--engine.slow-block-threshold`) that emits a structured `warn!` log with detailed timing, state-operation counts, and cache hit-rate metrics for blocks whose total processing time exceeds the threshold. Introduced `ExecutionTimingStats`, `CacheStats`, `StateProviderStats`, and `SlowBlockInfo` types to carry execution statistics from block validation through persistence, and refactored `PersistenceResult` to carry commit duration alongside the last persisted block.
