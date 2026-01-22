---
title: Experiment with cached hashed cursor for the trie node iterator
labels:
    - A-trie
    - C-perf
    - M-prevent-stale
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 17920
synced_at: 2026-01-21T11:32:15.983445Z
info:
    author: shekhirin
    created_at: 2025-03-25T20:28:59Z
    updated_at: 2025-05-06T12:24:59Z
---

### Describe the feature

Ref https://github.com/paradigmxyz/reth/blob/cd61097901823d09ab953b8b7a0839decf4e08ca/crates/trie/trie/src/hashed_cursor/cached.rs

---

### Profile from Base 

<img width="1280" alt="Image" src="https://github.com/user-attachments/assets/0c015b38-9a87-41c2-98d3-54a42625b6b4" />

### Logs
```console
2025-03-25T20:25:20.693369Z DEBUG trie::hashed_cursor::cached: CachedHashedCursorFactoryCache raw stats account_seeks_hit=155896 account_seeks_total=166921 account_nexts_hit=1198 account_nexts_total=8990 account_seeks_exact_size=99955 account_seeks_inexact_size=99875 account_nexts_size=7785

2025-03-25T20:25:20.693386Z DEBUG trie::hashed_cursor::cached: CachedHashedCursorFactoryCache hitrates account_seeks_hitrate=0.9339507910927924 account_nexts_hitrate=0.1332591768631813
2025-03-25T20:25:20.743523Z  INFO engine::tree: State root task finished state_root=0x729d8383e4fe8caf69ad273122c1a8045b48f04622fe35e0c9c7496e25209b92 elapsed=55.814339ms

2025-03-25T20:25:20.743879Z  INFO reth_node_events::node: Block added to canonical chain number=28072486 hash=0x8fd979cce75354cc29256bd4797baf25485bf9e5413d1d5d25fae43f633c6f57 peers=32 txs=258 gas=65.44 Mgas gas_throughput=439.35 Mgas/second full=56.4% base_fee=0.00gwei blobs=0 excess_blobs=0 elapsed=148.953431ms

2025-03-25T20:25:20.744604Z  INFO reth_node_events::node: Canonical chain committed number=28072486 hash=0x8fd979cce75354cc29256bd4797baf25485bf9e5413d1d5d25fae43f633c6f57 elapsed=319.332µs

2025-03-25T20:25:20.986384Z DEBUG trie::hashed_cursor::cached: Updated cached account inexact seeks elapsed=242.239298ms noops=99492 updated=187 deleted=0 invalidated=113
```

---

The hitrate for seeks is very good and consistently at about 90%. The problem with this implementation is that querying moka's cache takes too much time, because it also needs to update TTLs etc on every `get`. Due to that, there's no improvement over normal implementation that goes to the database every time.

We need to experiment with better caching here, and make the `get`s faster.

### Additional context

_No response_
