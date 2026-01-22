---
title: 'feat(cli): proof and trie node pretty printer'
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
synced_at: 2026-01-21T11:32:16.010897Z
info:
    author: yongkangc
    created_at: 2025-12-09T04:14:22Z
    updated_at: 2025-12-09T10:11:54Z
---

### Background

Working with RLP encoded proofs/witnesses requires manual decoding. When we have proofs from a witness or `eth_getProof` call, there's no easy way to explore them.

Parent: #20212

### Scope

* Offline command: takes RLP proofs from stdin, pretty-prints to stdout
* `reth db decode-node` or similar - decode trie node from RLP
* Show branch node masks with bit interpretations (which nibbles have children)

**Approach:**
1. Parse RLP from stdin
2. Display with bit position interpretations (e.g., `0x280f [0,1,2,3,9,d]`)
