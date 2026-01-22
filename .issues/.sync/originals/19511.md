---
title: 'Tracking: Proof Calculation Rewrite'
labels:
    - A-engine
    - A-trie
    - C-perf
    - C-tracking-issue
assignees:
    - mediocregopher
milestone: v1.11
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.002112Z
info:
    author: mediocregopher
    created_at: 2025-11-05T14:07:58Z
    updated_at: 2026-01-21T10:19:25Z
---

## Background

We use merkle proofs as a means of populating the sparse trie during payload validation. For every account or storage slot which is updated, we fetch a proof for that slot within the accounts or storage trie, respectively. This proof contains all nodes in the trie from the root to that leaf. These nodes are passed to the sparse trie in order to "reveal" the nodes to it. These revealed nodes are retained in memory throughout validation of the block payload. These revealed nodes allow the sparse trie to perform modifications to the trie, such as adding, updating, and removing leaves, in-memory with minimal overhead.

### Partial Proofs

Partial proof targets are proof targets which indicate that we want a proof for key N, but only those nodes whose path length is greater than some threshold. For example:

Let's say a transaction produces a proof target of `0x123abc`, and that proof then reveals nodes `0x`, `0x12`, `0x123a`, and `0x123ab`. A subsequent transaction then produces the proof target `0x123aff` in the same trie, with the proof revealing nodes `0x`, `0x12`, `0x123a`, and `0x123af`. Note that the first 3 nodes in each proof are the exact same nodes (`0x`, `0x12`, `0x123a`), meaning the work required to calculate those nodes was duplicated between the two proofs.

In the example above, because we already had nodes `0x`, `0x12`, and `0x123a`, and all of those share a prefix with the second proof target `0x123aff`, we could use a partial proof target of `[0x123aff, 4]` to indicate we only want to calculate proof nodes of key `0x123aff` with path length greater than 4.

## Summary

We are writing a new implementation of proof calculation, leaving the existing implementation untouched. Once completed we will use the new proof calculator in the state root task (sparse trie). If there are any bugs during payload validation reth will automatically fallback to the legacy StateRoot which has been left untouched.

Relates to https://linear.app/tempoxyz/project/proof-v2-baffb220bfbc/issues

Scope:

* A from-scratch implementation which does not use HashBuilder, TrieWalker, or TrieNodeIter.

* Does not rely on tree_mask for iterating over Accounts/StoragesTrie tables.

* Accepts a new ProofTarget type which includes a minimum path length for partial proofs; support for partial proofs will be implemented but not yet used.

* Allows for re-use of database cursors across multiple proof generations, as well as any other resources allocated internally (e.g. Vecs or hashmaps).

* Integrated into proof tasks used by state root task.

* Proptests to compare legacy proof calculation with new one.

* Tracing and metrics for debugging and monitoring new implementation.

### Additional context

_No response_
