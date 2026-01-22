---
title: Experiment with smarter proof chunking logic
labels:
    - A-trie
    - C-enhancement
    - C-perf
    - M-prevent-stale
assignees:
    - Andrurachi
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
parent: 17920
synced_at: 2026-01-21T11:32:15.996773Z
info:
    author: mediocregopher
    created_at: 2025-08-25T12:27:20Z
    updated_at: 2026-01-19T12:21:00Z
---

### Describe the feature

We chunk proof tasks into many small ones in order to parallelize DB accesses.

https://github.com/paradigmxyz/reth/blob/f3c2a3dc2706bb9ace87b130cf13730301135a0f/crates/engine/tree/src/tree/payload_processor/multiproof.rs#L833-L851

However the chunking logic is fairly basic:

https://github.com/paradigmxyz/reth/blob/f3c2a3dc2706bb9ace87b130cf13730301135a0f/crates/trie/common/src/proofs.rs#L111-L118

Given this logic and a chunk size of N, we could end up in a situation where accountA has N/2 proofs, and accountB has N proofs. In this situation the first chunk would have N/2 accountA proofs and N/2 accountB proofs, and the second chunk would have N/2 accountB proofs. Ideally all of accountB's proofs would be included in its own chunk; this would reduce the number db cursors required to be created.

Achieving this could be done by producing the `flattened_targets` ordered by storage size, or by adding some intelligent threshold size check to the chunker itself at the boundaries between accounts.

It's unclear if this would lead to any real performance benefit, but it's worth trying.

### Additional context

_No response_
