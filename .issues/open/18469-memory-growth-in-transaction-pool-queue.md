---
title: Memory growth in Transaction Pool Queue
labels:
    - A-tx-pool
    - C-bug
assignees:
    - yongkangc
type: Bug
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.998467Z
info:
    author: yongkangc
    created_at: 2025-09-15T13:04:19Z
    updated_at: 2025-09-15T13:06:32Z
---

### Describe the bug

sounds like the issue for the memory growth is just because the transactions are in the pool queue and cannot be processed, and that the issue is likely coming from the pool (instead of rpc / engine)

 I believe the txs are sitting in this channel
https://github.com/paradigmxyz/reth/blob/119ed881ec66aaf266c9c23554dada12eac36c05/crates/transaction-pool/src/validate/task.rs#L80-L92


what I think happens is that we basically queue up a shitton of eth_sendraw + validations
that then need a ton of time to eventually make it into the pool
likely a combination of suboptimal batch between rpc -> validation -> pool insert

### Steps to reproduce

```
cargo r --release -- node --http --http.api all --ws --ws.api all --dev --dev.block-time 12s --txpool.additional-validation-tasks 3 --engine.disable-caching-and-prewarming

```

```
cargo run --release -- --ipc /tmp/reth.ipc --ws [ws://127.0.0.1:8546](ws://127.0.0.1:8546/)

```

 identified most allocs from
 https://github.com/paradigmxyz/reth/blob/482f4557eb84cd49032d519b9593cf09a6dd9e33/crates/trie/db/src/hashed_cursor.rs#L42
Called from here https://github.com/paradigmxyz/reth/blob/482f4557eb84cd49032d519b9593cf09a6dd9e33/crates/trie/db/src/hashed_cursor.rs#L42
