# State Root Calculation of Engine Payloads

The heart of Reth is the Engine, which is responsible for driving the chain forward.
Each time it receives a new payload ([engine_newPayloadV4](https://github.com/ethereum/execution-apis/blob/main/src/engine/prague.md#engine_newpayloadv4)
at the time of writing this document), it does a bunch of validations, executes the block
contained in the payload, calculates the [MPT](https://ethereum.org/en/developers/docs/data-structures-and-encoding/patricia-merkle-trie/)
root of the new state, compares it with the one received in the block header,
and then considers the block committed.

This document describes the lifecycle of a payload with the focus on state root calculation,
from the moment the payload is received, to the moment we have a new state root.

We will look at the following components:
- [Engine](#engine)
- [State Root Task](#state-root-task)
- [Multiproof Manager](#multiproof-manager)
- [Sparse Trie Task](#sparse-trie-task)

## Engine

![Alt text](./mermaid/engine.mmd.svg)

It all starts with the `engine_newPayload` request coming from the [Consensus Client](https://ethereum.org/en/developers/docs/nodes-and-clients/#consensus-clients).

We extract the block from the payload, and eventually pass it to the `EngineApiTreeHandler::insert_block_inner`
method https://github.com/paradigmxyz/reth/blob/2ba54bf1c1f38c7173838f37027315a09287c20a/crates/engine/tree/src/tree/mod.rs#L2359-L2362

First, we spawn the [State Root Task](#state-root-task) thread, which will receive the updates from
execution and calculate the state root https://github.com/paradigmxyz/reth/blob/2ba54bf1c1f38c7173838f37027315a09287c20a/crates/engine/tree/src/tree/mod.rs#L2449-L2458

Then, we do two things with the block:
1. Start prewarming each transaction in a separate thread ("Prewarming thread" on the above diagram) https://github.com/paradigmxyz/reth/blob/2ba54bf1c1f38c7173838f37027315a09287c20a/crates/engine/tree/src/tree/mod.rs#L2490-L2507
    - Each transaction is optimistically executed in parallel with each other on top of the previous block,
    but the results are not committed to the database.
    - All accounts and storage slots that were accessed are cached in memory, so that the actual execution
    can use them instead of going to the disk.
    - All modified accounts and storage slots are sent as `StateRootMessage::PrefetchProofs`
    to the [State Root Task](#state-root-task).
    - Some transactions will fail, because they require the previous transactions to be executed first.
2. Execute transactions sequentially https://github.com/paradigmxyz/reth/blob/2ba54bf1c1f38c7173838f37027315a09287c20a/crates/engine/tree/src/tree/mod.rs#L2523
    - Transactions are executed one after another, and use the accounts and storage slots cache
    from the prewarming step, if it's available.
    - All modified accounts and storage slots are sent as `StateRootMessage::StateUpdate`
    to the [State Root Task](#state-root-task).
    - When all transactions are executed, the `StateRootMessage::FinishedStateUpdates` is sent
    to the [State Root Task](#state-root-task).

Eventually, the Engine will receive the `StateRootMessage::RootCalculated` message from
the [State Root Task](#state-root-task) thread, and will use the new state root to decide
the `engine_newPayload` response.


## State Root Task

![Alt text](./mermaid/state-root-task.mmd.svg)

## Multiproof Manager

![Alt text](./mermaid/multiproof-manager.mmd.svg)

## Sparse Trie Task

![Alt text](./mermaid/sparse-trie-task.mmd.svg)

