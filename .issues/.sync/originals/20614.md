---
title: 'reth-bench: import/export block ranges + big blocks'
labels:
    - A-reth-bench-compare
    - C-enhancement
assignees:
    - Rjected
type: Task
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20607
synced_at: 2026-01-21T11:32:16.017264Z
info:
    author: mediocregopher
    created_at: 2025-12-23T15:59:35Z
    updated_at: 2026-01-19T11:09:30Z
---

### Describe the feature

* reth-bench should be modified so that it can export a range of payloads to a file, and then to use an exported range for `new-payload-fcu` calls.

* reth-bench should be modified so that when exporting a range of blocks, it instead merges blocks into Big Blocks, ie it exports 1 block for every N real blocks, with the 1 block being composed of the txs of the N real blocks.

* reth-bench-compare should be modified so that it can also use an exported range for its reth-bench calls.

### Additional context

Prior work:

An existing branch exists which modifies reth-bench to force reorgs: https://github.com/paradigmxyz/reth/tree/mediocregopher/reth-bench-reorgs

It accomplishes this by building new blocks from the txs of existing blocks. Something like this could be done to accomplish building the Big Blocks.
