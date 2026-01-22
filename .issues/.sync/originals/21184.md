---
title: 'perf(storage): Add bloom filter for empty storage slots'
labels:
    - C-perf
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:16.020112Z
info:
    author: gakonst
    created_at: 2026-01-19T10:02:13Z
    updated_at: 2026-01-19T10:02:13Z
---

## Summary

Inspired by Nethermind's FlatDB approach ([PR #9854](https://github.com/NethermindEth/nethermind/pull/9854)), we could add a dedicated bloom filter to short-circuit lookups for empty storage slots.

## Motivation

From Nethermind's analysis:
> "30%-40% of slot reads are empty slots. These can be filtered via a large (around 2-3GB bloom filter)... up to 20% almost free mgas/sec on some block ranges."

RocksDB's built-in bloom filters are per-SST file and not as fast as a dedicated in-memory bloom for this specific use case.

## Proposed Changes

1. Maintain a global bloom filter (~2-3GB) tracking which storage slots exist
2. Check bloom filter before RocksDB lookup for storage reads
3. If bloom says "not present", return empty immediately (no I/O)
4. Update bloom on storage writes (set/delete)

## Trade-offs

From Nethermind's experience:
- **Pro**: Up to 20% mgas/sec improvement on some block ranges
- **Con**: Heavy write amplification to maintain the bloom
- **Con**: Significant memory overhead (2-3GB)
- **Con**: Unclear if always faster - depends on workload

## Implementation Notes

- Bloom filter should be persisted to avoid rebuild on restart
- Consider making this optional/configurable
- May want to experiment with bloom size vs false positive rate

## Reference

- Nethermind FlatDB PR: https://github.com/NethermindEth/nethermind/pull/9854
- Relevant code: `BasePersistence` with `BloomFilter` integration

## Parent Issue

Part of #17920
