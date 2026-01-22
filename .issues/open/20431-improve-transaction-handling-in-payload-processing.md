---
title: Improve transaction handling in payload processing
labels:
    - A-engine
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.014758Z
info:
    author: shekhirin
    created_at: 2025-12-16T19:51:42Z
    updated_at: 2026-01-19T13:07:33Z
---

There are several issues with how transactions are handled during payload processing that could be improved for correctness and efficiency.

### Issues

#### 1. Incorrect transaction index in prewarm task
- [`crates/engine/tree/src/tree/payload_processor/prewarm.rs:183`](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/prewarm.rs#L183)

The indexes used are not actual order indexes. The `pending` channel should send `(idx, tx)` tuples to preserve correct ordering.

#### 2. Missing `is_system_tx` method on transaction type
- [`crates/engine/tree/src/tree/payload_processor/prewarm.rs:185`](https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/payload_processor/prewarm.rs#L185)

The system transaction check (`tx.ty() > MAX_STANDARD_TX_TYPE`) is not accounting for transaction types introduced by Reth SDK chains. This should be exposed as an `is_system_tx()` method on the transaction type itself.

#### 3. Redundant transaction decoding
- [`crates/ethereum/payload/src/validator.rs:79`](https://github.com/paradigmxyz/reth/blob/main/crates/ethereum/payload/src/validator.rs#L79)

Transactions are already decoded by `tx_iterator` but not passed to `try_into_block_with_sidecar`, causing redundant decoding work.
