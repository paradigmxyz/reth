# Reth Historical Proof Storage

## Current Implementation

Currently, only key-value pairs are stored for historical state. This allows for efficient serving of most RPCs except those that need historical state.

For the historical state RPCs, Reth needs to traverse half the entire tree and hash the relevant nodes to prove the state. This results in slow proof times.

## Use Cases

We need to support these RPCs

- `eth_getProof`: proves a small number of key-value pairs against an old block state root
    - Many RPC providers want to provide this RPC for users, but currently can't with Reth.
- `debug_executePayload`: executes an arbitrary block and returns the prestate and proof of each needed key-value pair
    - Optimism chains need this RPC to execute possibly invalid blocks on top of historical state as part of the fault dispute game.
- `debug_executionWitness`: similar to executePayload but only with blocks in the chain

Based on these, we need a way to fetch proofs that:
- can efficiently prove a decently large number of slots (1,000-10,000) within a few seconds over an arbitrary number of blocks (1M+ for historical RPC providers)
- can prove any slot in the state trie, not only recently accessed/modified slots

If we had a way of storing trie nodes, we could potentially improve the efficiency of these proofs significantly. Instead of having to calculate the entire left subtrie (assuming the key being proven is on the right), we could just look it up.

This means higher storage requirements, but faster proof generation times.

## Baseline implementation

Keys are the concatenation of the path and block number where the trie node was created:

```
#<optional_account_hash>-<trie_path>-<block_number>
```

The values of the table are compact branch nodes, similar to the AccountsTrie and StorageTrie tables. The value can also be empty if the node was deleted at a certain block.

We also need a secondary index from block number to the keys created in that block number.

### How trie nodes are stored incrementally

We just have to load the entire `AccountsTrie` and `StorageTrie` data into a new `AccountsTrieHistory` and `StorageTrieHistory` table. These will have block number set to 0. We will also separately keep track of minimum block number. This allows us to prune without updating the block number of the earliest state.

As we process blocks, we simply apply `TrieUpdates` in the same way as the existing tables. Deletions in the historical tables will just be inserting an empty value.

#### Reorgs

When a block is removed from the chain, we simply remove all of the keys with that block number or above and then reapply the changes from the new chain, storing intermediate nodes in the normal way.

## Implementing Proof-Serving Interfaces

### TrieCursor

TrieCursor can be implemented as an iterator over trie nodes. Because the table is sorted by path, we should be able to efficiently find the next node by seeking to the last block number of the node (latest value), and then seeking to the next node.

If the latest value of the next node is empty, the node was deleted at that block, so we can repeat the process until we reach the end of the table or find a non-deleted node.

### HashedAccountCursor

This can be implemented in the same way as normal Reth. Reth already tracks history of leaf nodes, so we can use the existing table and implementation.

## Implementation

### Reth Execution Extension

We'll build a Reth execution extension that will:

- Save the initial state from existing tables (AccountsTrie and StorageTrie).
- Listen for MerkleExecute events and store the TrieUpdates in the new tables.
- Serve proof requests from the new tables.
    - Open question: currently, ExExes can't do this because they don't have access to the proof serving interfaces. How do we serve proof RPCs? Maybe we can create a sidecar process that handles proof requests and serves the response from existing tables.

## Optimizations

### BranchNodeCompact vs BranchNode

We could store all RLP-encoded trie nodes directly in the database, but this results in poor write performance. Almost every leaf update would require updating a branch (almost doubling the number of writes).

We can benefit by the optimization Reth makes by only storing branch nodes that have other branch nodes as children. Note that it's important to store deletions in this case because there's a chance a branch node is replaced by an extension node.

### Pruning

To prune, we can remove any trie nodes that are no longer needed. To prune block N-1 when we have state from block N-1 to the tip, we can simply fetch all changes in block N-1 and apply them to the base state with block number set to 0. This only requires reading/writing the number of nodes that were modified in block N-1.

The special block number 0 represents the earliest state and should have the most number of trie nodes stored. It doesn't actually mean this is the state at block number 0; instead it means the earliest state we have. The actual block number is stored separately so we don't have to update all these rows every time we want to prune a block.

### Batching all trie node changes to every Nth block

Instead of storing every single trie node change, we can batch changes together (10-20 blocks) and store the aggregated trie changes from these blocks. To access an intermediate trie node, we can reconstruct its state by applying the changes from the relevant blocks (we already have the leaf nodes that were changed as part of AccountHistory and StorageHistory).

## Overall Recommendation

- Store trie nodes in a compact format to reduce storage requirements.
- Implement a pruning-friendly numbering strategy to minimize incremental work.
- In the future, allow batching trie node changes to reduce storage requirements.