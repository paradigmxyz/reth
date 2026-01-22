---
title: Add support for engine_executeStatelessPayloadVX Engine RPCs
labels:
    - A-rpc
    - C-enhancement
    - D-good-first-issue
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.97884Z
info:
    author: Akagi201
    created_at: 2024-11-22T05:16:38Z
    updated_at: 2025-08-04T23:00:59Z
---

### Describe the feature

`engine_executeStatelessPayloadV1`,`engine_executeStatelessPayloadV2`,`engine_executeStatelessPayloadV3`,`engine_executeStatelessPayloadV4`, engine RPC methods are already implemented in [go-ethereum](https://github.com/ethereum/go-ethereum/blob/e3d61e6db028c412f74bc4d4c7e117a9e29d0de0/eth/catalyst/api.go#L767) node, which is useful to execute a witness data to calculate block header fields.

### Additional context

_No response_
