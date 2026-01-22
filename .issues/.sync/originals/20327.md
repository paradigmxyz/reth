---
title: Make blockstream in debugclient configurable
labels:
    - A-sdk
    - C-enhancement
assignees:
    - 0xKarl98
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.013336Z
info:
    author: mattsse
    created_at: 2025-12-12T10:36:45Z
    updated_at: 2025-12-12T10:41:22Z
---

### Describe the feature

for debug purposes we use

https://github.com/paradigmxyz/reth/blob/ed3a8a03d59bd6db62972f4c87b608bc217b0b29/crates/consensus/debug-client/src/providers/rpc.rs#L14-L18

that subscribes to blocks from a node, this is currently used by all `DebugNode` via:

https://github.com/paradigmxyz/reth/blob/ed3a8a03d59bd6db62972f4c87b608bc217b0b29/crates/node/builder/src/launch/debug.rs#L57-L66

ideally this is can be configured, so we can change the debug trait so that we replace the `rpc_to_primitive_block` with something that returns 

https://github.com/paradigmxyz/reth/blob/ed3a8a03d59bd6db62972f4c87b608bc217b0b29/crates/consensus/debug-client/src/client.rs#L16-L16

instead, then the default impl would just return something like `-> impl BlockProvider { RpcBlockProvider }` in the trait.

@0xKarl98 

### Additional context

_No response_
