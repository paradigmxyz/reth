---
title: Add example for custom payload-builder/sequencer
labels:
    - A-sdk
    - C-enhancement
assignees:
    - Peponks9
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.990835Z
info:
    author: mattsse
    created_at: 2025-06-23T15:40:22Z
    updated_at: 2025-08-27T18:14:10Z
---

### Describe the feature

a crucial element for L2s is the payload-builder/sequencer logic.

For Op we now have 

https://github.com/paradigmxyz/reth/blob/dc67f0237ff01048e79aec2f6f346ad378dc3afc/crates/optimism/payload/src/builder.rs#L135-L151

and other modifications,
we want a dedicated example crate
that creates a custom payloadbuilder logic, with features like,
e.g. prioritized block space

which is already possible for Op:

https://github.com/paradigmxyz/reth/blob/dc67f0237ff01048e79aec2f6f346ad378dc3afc/crates/optimism/payload/src/builder.rs#L402-L411

but we can showcase existing primitives like

https://github.com/paradigmxyz/reth/blob/dc67f0237ff01048e79aec2f6f346ad378dc3afc/crates/payload/util/src/transaction.rs#L56-L56

or introduce new ones depending on the use case

## TODO
* create new example crate with custom payloadbuilder(builder) implementations

### Additional context

_No response_
