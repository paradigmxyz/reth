---
title: Add example that demonstrates and documents --engine.persistence-threshold 0 and its impacts
labels:
    - C-enhancement
    - C-example
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.988167Z
info:
    author: mattsse
    created_at: 2025-06-05T09:01:42Z
    updated_at: 2025-07-23T09:08:36Z
---

### Describe the feature

ref #16511

the way these settings work is that they control how early we flush a new payload to disk, the default threshold is 2 blocks, motivated by efficient reorg handling, meaning that 2 block deep reorgs can always be handled in memory.
The impact of this is however that if reth db's is accessed externally, the disk will always trail by two blocks, this is what #16511 describes.

https://github.com/paradigmxyz/reth/blob/63cc4eccadc5149ac725746cb2ef1c629407de10/crates/node/core/src/args/engine.rs#L15-L21

we want an example crate that is similar to the db-access example

https://github.com/paradigmxyz/reth/blob/63cc4eccadc5149ac725746cb2ef1c629407de10/examples/db-access/src/main.rs#L22-L22

so that we can document this behaviour properly.

## TODO
* add an example and additional docs for this behaviour

### Additional context

_No response_
