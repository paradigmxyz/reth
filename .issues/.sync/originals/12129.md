---
title: 'bug(trie): invalid trie masks'
labels:
    - A-trie
    - C-bug
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.977974Z
info:
    author: rkrasiuk
    created_at: 2024-10-28T09:38:14Z
    updated_at: 2024-11-27T09:50:55Z
---

## Description

Currently, we commit intermediate trie nodes with invalid trie masks (issue to be amended with the cause after investigation cc @fgimenez). This causes trie walker to have invalid stack when it calls `self.consume_node` and pushes a subnode to the stack which does not share a common prefix with the previous last node on the stack. 
https://github.com/paradigmxyz/reth/blob/8605d04a09679904ef25594729ac6b83dfcacfcb/crates/trie/trie/src/walker.rs#L135-L148

Once the subnode is done processing, it comes back to a previous one and returns a node key that is out of order causing has builder to panic.

## Addition Info

Issue reports: https://github.com/paradigmxyz/reth/issues/11471, https://github.com/paradigmxyz/reth/issues/11955 
