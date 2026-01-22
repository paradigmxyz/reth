---
title: Improve OP txpool batch validation
labels:
    - A-op-reth
    - C-enhancement
    - C-perf
    - D-good-first-issue
assignees:
    - Aideepakchaudhary
    - emhane
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.980996Z
info:
    author: mattsse
    created_at: 2025-01-21T15:10:15Z
    updated_at: 2025-03-10T12:16:05Z
---

### Describe the feature

the opvalidator currently does:

https://github.com/paradigmxyz/reth/blob/6dabd5244ebe206eddea1679996c3f9cc78ffb8b/crates/optimism/node/src/txpool.rs#L411-L411

and delegates to the regular eth validator:

https://github.com/paradigmxyz/reth/blob/6dabd5244ebe206eddea1679996c3f9cc78ffb8b/crates/optimism/node/src/txpool.rs#L334-L346

this is suboptimal for batches because the eth validator is optimized for batches.

we can improve this by delegating to https://github.com/paradigmxyz/reth/blob/6dabd5244ebe206eddea1679996c3f9cc78ffb8b/crates/transaction-pool/src/validate/eth.rs#L498-L499 first and then perform the additional OP checks.

## TODO
* split delegated validation from validate_one https://github.com/paradigmxyz/reth/blob/6dabd5244ebe206eddea1679996c3f9cc78ffb8b/crates/optimism/node/src/txpool.rs#L346-L346
* improve Op validate_batch

cc @hai-rise 

### Additional context

_No response_
