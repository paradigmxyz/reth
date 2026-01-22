---
title: 'perf(trie): Separate top-level trie nodes into dedicated RocksDB column'
labels:
    - A-rocksdb
    - C-perf
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.019949Z
info:
    author: gakonst
    created_at: 2026-01-19T10:01:59Z
    updated_at: 2026-01-21T10:26:52Z
---

## Summary

Inspired by Nethermind's FlatDB approach ([PR #9854](https://github.com/NethermindEth/nethermind/pull/9854)), we could separate top-level trie nodes (≤5 nibbles depth) into a dedicated RocksDB column with higher block cache priority.

## Motivation

From Nethermind's analysis:
- Top-level state nodes (≤5 nibbles) represent a small portion of the trie (~648MB) but are accessed frequently
- They are ~5x larger than deeper nodes due to pruning behavior
- Separating them improves cache hit rates and read locality
- Reduces duplicated keys in the top section and keeps out-of-order keys to ~0.03-0.07%

## Proposed Changes

1. Create a new RocksDB column `StateTopNodes` for state trie nodes where path nibble count ≤ 5
2. Key format: `[path_prefix:3 bytes][path_length:1 byte]` (since max 5 nibbles needs only 3 bytes)
3. Configure with dedicated block cache allocation (e.g., 30% of state cache budget)
4. Keep existing `StateNodes` column for deeper nodes

## Expected Benefits

- Better block cache utilization for hot paths
- Reduced compaction interference between frequently and rarely accessed nodes
- Potential CPU savings from smaller column to search

## Reference

- Nethermind FlatDB PR: https://github.com/NethermindEth/nethermind/pull/9854
- Key insight from their `StateTopNodes` column design

## Parent Issue
Closes RETH-142
Part of #17920
