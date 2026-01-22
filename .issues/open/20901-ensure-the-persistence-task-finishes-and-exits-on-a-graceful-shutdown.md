---
title: Ensure the persistence task finishes and exits on a graceful shutdown
labels:
    - A-db
    - A-engine
assignees:
    - stevencartavia
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.018335Z
info:
    author: joshieDo
    created_at: 2026-01-09T21:11:37Z
    updated_at: 2026-01-09T21:37:17Z
---

### Describe the change

We should ensure the persistence task finishes and exits on a graceful shutdown to avoid any storage inconsistencies needing to be healed or loss of in-memory blocks (eg. k8s restarts)

### Additional context

basically we should invoke this feature by default now on graceful shutdown

https://github.com/paradigmxyz/reth/blob/e86c5fba535760e2bd69c94647eede7465454900/crates/node/builder/src/rpc.rs#L1457-L1463


https://github.com/paradigmxyz/reth/blob/e86c5fba535760e2bd69c94647eede7465454900/crates/node/builder/src/launch/engine.rs#L376-L376

so we should spawn this service as graceful shutdown task instead, and then always trigger persistence on shutdown

https://github.com/paradigmxyz/reth/blob/e86c5fba535760e2bd69c94647eede7465454900/crates/node/builder/src/launch/engine.rs#L310-L317

https://github.com/paradigmxyz/reth/blob/e86c5fba535760e2bd69c94647eede7465454900/crates/tasks/src/lib.rs#L505-L505

then perhaps we don't need the `EngineShutdown` type anymore perhaps
