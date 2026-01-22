---
title: Chunking Dynamically instead of using fixed limits
labels:
    - A-engine
    - C-perf
    - M-prevent-stale
    - S-needs-triage
    - S-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
parent: 17920
synced_at: 2026-01-21T11:32:16.001381Z
info:
    author: yongkangc
    created_at: 2025-10-23T11:48:37Z
    updated_at: 2025-12-12T04:10:24Z
---

### Describe the feature

Currently we are hardcoding the chunk size here
https://github.com/paradigmxyz/reth/blob/60e3eded5e0a19dd03b9efb16e08ca58ccb47eb9/crates/engine/primitives/src/config.rs#L32-L33

And we only want to chunk when there are available workers. This has a huge impact for PST, as seen from https://github.com/paradigmxyz/reth/pull/18727/files because it's just harmful to chunk when you can't process the multiproof immediately, because it will go into the queue and the latency will be higher.

Chunking rules: 
https://github.com/paradigmxyz/reth/blob/1dc50aaf17fd5cc0dfe55629186972dc7c72b0eb/crates/engine/primitives/src/config.rs#L32-L33
https://github.com/paradigmxyz/reth/blob/1dc50aaf17fd5cc0dfe55629186972dc7c72b0eb/crates/trie/common/src/proofs.rs#L94-L114

**The key question is how can we chunk smarter?** 

some ideas could be possible such as: 

1. Use the number of idle workers to calculate a dynamic chunk size instead of hardcoding one.
2. Having an adaptive chunking that allows us to chunk based on size of blocks, available workers
3. adapting chunking size based on queue depth
4. size based chunking (chunking only if > size)

And possibly moving decision to chunk upstream to Proof task where the Proof task should do the chunking to distribute the work among workers (or something similar)

### Additional context

_No response_
