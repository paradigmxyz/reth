---
title: 'feat(cli): output expected DB trie nodes'
labels:
    - A-cli
    - A-trie
assignees:
    - yongkangc
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20212
synced_at: 2026-01-21T11:32:16.011059Z
info:
    author: yongkangc
    created_at: 2025-12-09T04:14:26Z
    updated_at: 2025-12-09T10:04:46Z
---

### Background

No tool to output expected DB trie nodes for a subsection of any trie. Useful for comparing against actual stored nodes when debugging inconsistencies.

Parent: #20212

### Scope

* `reth db trie expected --account <addr> --path-prefix <path>`
  - Output expected DB trie nodes for a subsection based on hashed state
  - JSON output mode for tooling

### Possible Approach

Use `StateRootBranchNodesIter` pattern to compute expected nodes from `HashedStorages`.

* [`verify.rs`](https://github.com/paradigmxyz/reth/blob/main/crates/trie/trie/src/verify.rs) - `StateRootBranchNodesIter` computes expected nodes from hashed state
* [`nibbles.rs`](https://github.com/paradigmxyz/reth/blob/main/crates/trie/common/src/nibbles.rs) - path prefix matching utilities
