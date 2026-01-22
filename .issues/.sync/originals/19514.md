---
title: 'Proof Calc Rewrite: Integration (CLI flag)'
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
synced_at: 2026-01-21T11:32:16.002345Z
info:
    author: mediocregopher
    created_at: 2025-11-05T14:11:51Z
    updated_at: 2026-01-21T10:09:25Z
---

### Describe the feature

Integrate the new proof calculator into the proof tasks used by the state root task (sparse trie) during payload validation.

Integration will require:

* Using storage proof workers to calculate account storage roots, rather than doing so synchronously within account proofs.

* Caching account storage roots across account proof workers.

Initially the new implementation should only be enabled via a CLI flag, which will later be made into a default.

https://linear.app/tempoxyz/issue/RETH-167/integration-cli-flag

### Additional context

_No response_
