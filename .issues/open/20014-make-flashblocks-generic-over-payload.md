---
title: Make Flashblocks Generic over Payload
labels:
    - A-op-reth
    - A-sdk
    - C-enhancement
assignees:
    - rezzmah
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.009283Z
info:
    author: rezzmah
    created_at: 2025-11-27T15:53:37Z
    updated_at: 2025-12-01T09:25:57Z
---

### Describe the feature

Currently, many teams are looking at pre-confirmations. Most solutions are variations of Flashblocks (e.g. Gattaca's Frags).
Rather than everyone recreating the wheel, I believe Reth would be the optimal place for a modular implementation.

The optimal modular solution would look as follows:
1. `rblib` for custom payload building with Flashblocks when the sequencer builds blocks. The Flashbots team sees no reason this could be uncoupled from the Optimism payload types. This would be responsible for transmitting encoded flashblocks over websocket.
2. Reth for ingesting and serving flashblocks state. High level, this involves
	1. Decoding JSON bytes into the `FlashBlock` type. This is hardcoded to `OpFlashblockPayload` which could be modularized. [src](https://github.com/paradigmxyz/reth/blob/12fd25892dc603ebc646a43c99b14400656098cb/crates/optimism/flashblocks/src/ws/decoding.rs#L18)
	2. Inserting the Flashblock(s) into a pending sequence state [src](https://github.com/paradigmxyz/reth/blob/118fd3b3720ca2c244d18102cc0c1632096b860f/crates/optimism/flashblocks/src/cache.rs#L79)
	3. Pending Block Building, responsible for creating a `PendingFlashBlock`. This seems fairly flexible already except requiring `NextBlockEnvCtx: From<OpFlashblockPayloadBase>`. I don't believe this is needed.  [src](https://github.com/paradigmxyz/reth/blob/118fd3b3720ca2c244d18102cc0c1632096b860f/crates/optimism/flashblocks/src/worker.rs#L66)
	4. Finally in the RPC layer, the `PendingFlashBlock` is observed and state is read from the internal `PendingBlock`. Both of these types don't seem to be Optimism specific. [src](https://github.com/paradigmxyz/reth/blob/85287698963e8640dd63995b49fff59096f21adc/crates/optimism/rpc/src/eth/mod.rs#L175)

Let me know if there's appetite for something like this and I can whip up a PR



### Additional context

_No response_
