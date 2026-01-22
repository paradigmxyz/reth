---
title: Document reth's tracing targets
labels:
    - C-docs
    - C-enhancement
    - D-good-first-issue
    - S-stale
assignees:
    - jenpaff
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.977776Z
info:
    author: Rjected
    created_at: 2024-10-22T23:48:24Z
    updated_at: 2025-05-25T14:11:42Z
---

We have a lot of different trace targets, and we don't have any broad overview of what trace targets someone might want to turn on if they're trying to debug a component or protocol.

We should list all specific log targets, for example some crates have log targets that are different than the crate name or are organized differently than just with crates:
https://github.com/paradigmxyz/reth/blob/527d344ddab7397ede9f06874b9a00746fa2fa41/crates/net/network/src/manager.rs#L722
https://github.com/paradigmxyz/reth/blob/527d344ddab7397ede9f06874b9a00746fa2fa41/crates/engine/tree/src/chain.rs#L127

The document should also contain a list of crates that typically do not use targets like this, for example with `reth_eth_wire`:
https://github.com/paradigmxyz/reth/blob/527d344ddab7397ede9f06874b9a00746fa2fa41/crates/net/eth-wire/src/ethstream.rs#L119-L122

The docs should try to explain whether or not the log level makes a difference, and what gets logged, for example:
* `reth_eth_wire` has `trace!` and `debug!` targets. It outputs RLPx session related information with `trace` targets, and mostly outputs `debug` logs if an error has occurred somewhere in the session
