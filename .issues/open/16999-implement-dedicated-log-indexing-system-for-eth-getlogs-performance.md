---
title: Implement dedicated log indexing system for `eth_getLogs` performance
labels:
    - A-db
    - A-rpc
    - C-enhancement
    - C-perf
assignees:
    - SkandaBhat
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.990515Z
info:
    author: thaodt
    created_at: 2025-06-23T11:01:20Z
    updated_at: 2025-09-06T08:29:17Z
---

### Describe the feature

Following [community discussions](https://x.com/sina_mahmoodi/status/1930565142183387627) about `eth_getLogs` performance differences between clients, I conducted deep research into Geth's and [Nethermind's](https://github.com/NethermindEth/nethermind/pull/8464/) indexing approaches. 
The findings confirm that micro-optimizations can only get so far - for 10x improvements, I think we need to add an (optional) dedicated logs index. geth's implementation demonstrates this with query times of 0.55s for 100k blocks compared to reth's 8.4s.

## Description
Current `eth_getLogs` implementation has several performance bottlenecks:
- Must read every block's receipts in the query range
- ~1-5% false positive rate requires full block processing (bloom filter limitations)
- **No dedicated indexing**: Even with recent concurrency improvements (PR #16441 and a pending #16675 ), still relies on O(n) bloom filter scanning
- Loading large numbers of blocks into memory

Recent benchmarks show stark performance differences:
- **Geth (with log index)**: 0.55s for 100k blocks, 22s for 2.5M blocks
- **Reth (current)**: 8.4s for 100k blocks, 211s for 2.5M blocks
- Performance gap: **15x slower for 100k blocks, 10x slower for 2.5M blocks**

This demonstrates that Geth's log indexer can answer WETH flow queries from the EF treasury in milliseconds, while reth requires several seconds for the same query.

## Proposed Solution

Implement a dedicated log indexing system inspired by successful approaches in geth ([`filtermaps`](https://github.com/ethereum/go-ethereum/blob/master/core/filtermaps/filtermaps.go) - [EIP-7745](https://eips.ethereum.org/EIPS/eip-7745) data-structure based only - not consensus changes).

### Key Features
1. Separate MDBX tables (dedicated index storage) for efficient log lookups
2. Support fast queries by address, topics, and block ranges
3. Reduce storage overhead while maintaining query performance
5. Transparent upgrade with fallback to existing implementation

### Additional context

_No response_
