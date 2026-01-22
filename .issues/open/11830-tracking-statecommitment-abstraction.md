---
title: 'Tracking: `StateCommitment` abstraction'
labels:
    - A-sdk
    - A-trie
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.977363Z
info:
    author: frisitano
    created_at: 2024-10-17T10:11:40Z
    updated_at: 2025-02-18T17:58:54Z
---

### Describe the feature

# Overview
This issue will be used to plan the implementation of the `StateCommitment` implementation. The `StateCommitment` will be used to provide an abstraction over the types associated with the state commitment. This will allow reth to be used with different state representations configured by the user. Initial POC implemented [here](https://github.com/paradigmxyz/reth/pull/11786).

# Plan
We will implement the `StateCommitment` via the following units of work:

- [x] Introduce `StateCommitment` trait, integrate with `NodeTypes` and provide MPT implementation. - #11842 
- [x] Introduce `StateCommitment` types within providers. - #12602
- [x] Introduce `HashedPostStateProvider` and refactor methods on `HashedPostState` and `PrefixSetLoader` to be generic over the `KeyHasher` - #12607
- [x] Introduce `HashedStorageProvider` - #12894
- [x] Introduce `KeyHasherProvider` - #12895 

Refactor codebase to leverage `StateCommitment` types for all operations:
- [ ] StateRoot - #12896
- [ ] StorageRoot
- [ ] StateProof
- [ ] StateWitness
- [ ] KeyHasher
- [ ] ParallelStateRoot

### Additional context

https://github.com/paradigmxyz/reth/pull/11786
```[tasklist]
### Tasks
- [ ] https://github.com/paradigmxyz/reth/pull/11842
- [ ] https://github.com/paradigmxyz/reth/pull/12602
- [ ] https://github.com/paradigmxyz/reth/pull/12607
- [ ] https://github.com/paradigmxyz/reth/pull/12894
- [ ] https://github.com/paradigmxyz/reth/pull/12895
- [ ] https://github.com/paradigmxyz/reth/pull/12896
```
