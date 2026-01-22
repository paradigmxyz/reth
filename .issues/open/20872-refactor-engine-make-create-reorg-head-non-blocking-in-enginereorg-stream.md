---
title: 'refactor(engine): make `create_reorg_head` non-blocking in `EngineReorg` stream'
labels:
    - C-enhancement
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018026Z
info:
    author: doocho
    created_at: 2026-01-09T05:55:55Z
    updated_at: 2026-01-11T13:53:48Z
---

### Describe the feature

The `EngineReorg` stream wrapper currently executes `create_reorg_head()`
synchronously inside `poll_next()`, which blocks the async runtime.


**Problem**
create_reorg_head() performs several blocking operations:
1. Database lookups (provider.block_by_hash(), provider.sealed_header_by_hash())
2. State provider creation (provider.state_by_block_hash())
3. EVM setup and block builder creation
4. Sequential transaction execution
5. Block finalization

While these operations run, the entire async runtime is blocked, preventing other tasks from making progress.

**Proposed Solution**
Use spawn_blocking with oneshot channel pattern, similar to the approach used in crates/payload/basic/src/lib.rs.

### Additional context

https://github.com/paradigmxyz/reth/blob/1866db4d500e25b5f51a6255d30fa5e8c6cf2528/crates/engine/util/src/reorg.rs#L159-L160
