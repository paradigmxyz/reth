---
title: 'Proof Calc Rewrite: Better usage of storage proof workers'
labels:
    - A-engine
    - A-trie
    - C-perf
assignees:
    - mediocregopher
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 19511
blocks:
    - 21242
synced_at: 2026-01-21T11:32:16.020809Z
info:
    author: mediocregopher
    created_at: 2026-01-21T10:15:01Z
    updated_at: 2026-01-21T10:16:49Z
---

### Describe the feature

In the current implementation of V2 account proofs we do the following:

* Dispatch all known storage proofs to proof workers
* Begin account proof, dispatching proofs for storage roots which have not yet been dispatched to storage workers
* Block on storage workers waiting for storage roots

This ends up slowing down the pipeline quite a bit, I suspect because we are waiting for all storage workers to finish the proofs they originally dispatched prior to the account proof being able to continue.

There are a few potential solutions:
* Set up a separate pool of workers just for storage roots which are needed for account proofs but don't have actual storage proof targets
* Don't dispatch storage proofs ahead of time, dispatch them as they are come across in the account proof. This requires ensuring that all accounts for which a storage proof is required have an account target in the same MuliProofTargets.
* Use synchronous account proof handling (basically the same behavior as legacy)

The task is to figure out the best solution and implement it.

related to RETH-170

### Additional context

_No response_
