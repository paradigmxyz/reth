---
reth-engine-primitives: minor
reth-engine-tree: major
reth-node-core: minor
reth-cli-commands: minor
---

Added persistence backpressure to the engine tree: when the canonical-minus-persisted block gap exceeds a configurable threshold (`--engine.persistence-backpressure-threshold`, default 16), the engine loop stalls on the persistence receiver instead of processing new incoming messages. Added CLI argument, cross-field validation, metrics (`backpressure_active`, `backpressure_stall_duration`), and tests.
