---
title: 'perf(trie): parallelize merge_ancestors_into_overlay for deferred trie computation'
labels:
    - C-perf
assignees:
    - mattsse
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
parent: 20607
synced_at: 2026-01-21T11:32:16.020259Z
info:
    author: mattsse
    created_at: 2026-01-19T13:19:18Z
    updated_at: 2026-01-21T10:27:30Z
---

## Description

`merge_ancestors_into_overlay` in `deferred_trie.rs` currently processes ancestors sequentially, which can become a bottleneck when rebuilding overlays with many in-memory ancestors.

## Profiler

See [Firefox Profiler](https://profiler.firefox.com/public/qd1rhjm07s2m0heagj0h2g4448ma8r9ndj9xd6r/flame-graph/?globalTrackOrder=201&hiddenGlobalTracks=01&hiddenLocalTracksByPid=1487349.2-0w2~1487349.1-xgxhxjwxly0wz4zazcwzjzlwAs&localTrackOrderByPid=1487349.1-xkxlzbxizaxhxgzcwzjzlwAsxmwz4Dk0wxfz5wz9zkAtwDixjDj&range=15647m2390&sourceViewIndex=882&symbolServer=http%3A%2F%2F127.0.0.1%3A3001%2F271h2d1w7a4xbb8v1ibqwc4pplmb4kxnb5add5s&thread=Dk&v=12) showing time spent in the merge path.

## Current behavior

```rust
for ancestor in ancestors {
    let ancestor_data = ancestor.wait_cloned();
    state_mut.extend_ref(ancestor_data.hashed_state.as_ref());
    nodes_mut.extend_ref(ancestor_data.trie_updates.as_ref());
}
```

This is O(n) sequential:
- Each `wait_cloned()` may block waiting for deferred computation
- Each `extend_ref()` runs sequentially

## Proposed optimization

1. **Parallel collection**: Call `wait_cloned()` on all ancestors in parallel via `par_iter()`
2. **Horizontal parallelism**: Use `rayon::join()` to merge hashed state and trie updates concurrently
3. **Tree reduction**: Add `merge_parallel()` methods to `HashedPostStateSorted` and `TrieUpdatesSorted` that use tree reduction for O(log n) merge depth

```
Before: A0 → A1 → A2 → ... → An (sequential)

After:
  1. par_iter: collect all ancestor data in parallel
  2. rayon::join:
     ├─ states: tree reduce (log n depth)
     └─ nodes:  tree reduce (log n depth)
```

<img width="1386" height="506" alt="Image" src="https://github.com/user-attachments/assets/54fa8240-b75d-4532-a897-ad885b446c7d" />

## Impact

This is a rare fallback path (normally parent overlay is reused), but when triggered with many ancestors it can be significant.

Closes RETH-145
