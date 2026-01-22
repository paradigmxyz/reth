---
title: State root calculation API that consumes the state provider
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.996229Z
info:
    author: hai-rise
    created_at: 2025-08-22T06:53:54Z
    updated_at: 2025-09-25T12:14:09Z
---

State root calculation is a common "final step" for many state providers, but the current `StateRootProvider` trait doesn't have an API to consume the provider itself:
https://github.com/paradigmxyz/reth/blob/a4dd305ee968566ab542abab5f036e3ec2bd8d7a/crates/storage/storage-api/src/trie.rs#L12-L40

This is wasteful for state providers with lots of in-memory cache, like `MemoryOverlayStateProviderRef`:
https://github.com/paradigmxyz/reth/blob/a4dd305ee968566ab542abab5f036e3ec2bd8d7a/crates/chain-state/src/memory_overlay.rs#L26-L29

For instance, when calculating the state root for new blocks (1000s of ERC-20 transactions each), our block builder spends ~33% of the time just preparing the trie input itself! 

<img width="1128" height="650" alt="Image" src="https://github.com/user-attachments/assets/dd4fe1be-18d6-415a-bfc3-66b05097862e" />

This process is very clone-heavy:
https://github.com/paradigmxyz/reth/blob/a4dd305ee968566ab542abab5f036e3ec2bd8d7a/crates/chain-state/src/memory_overlay.rs#L131-L135

https://github.com/paradigmxyz/reth/blob/a4dd305ee968566ab542abab5f036e3ec2bd8d7a/crates/chain-state/src/memory_overlay.rs#L57-L66

And would benefit a ton if we could consume things like `self.in_memory` and `self.trie_input` directly instead. 
