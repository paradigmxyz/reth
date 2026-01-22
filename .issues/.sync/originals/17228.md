---
title: Add Middleware generic to AuthServerConfig
labels:
    - A-rpc
    - C-enhancement
assignees:
    - iTranscend
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.992439Z
info:
    author: mattsse
    created_at: 2025-07-04T12:53:46Z
    updated_at: 2025-12-07T11:47:15Z
---

### Describe the feature

as prep for #17195

we should introduce a Middleware = Identity generic to:

https://github.com/paradigmxyz/reth/blob/342bab5e82bfb4d3fcf185fded4f8944f93e90eb/crates/rpc/rpc-builder/src/auth.rs#L24-L24

that we then use to bootstrap this:

https://github.com/paradigmxyz/reth/blob/342bab5e82bfb4d3fcf185fded4f8944f93e90eb/crates/rpc/rpc-builder/src/auth.rs#L54-L56

see also:

https://github.com/paradigmxyz/reth/blob/342bab5e82bfb4d3fcf185fded4f8944f93e90eb/crates/rpc/rpc-builder/src/lib.rs#L1241-L1242

https://github.com/paradigmxyz/reth/blob/342bab5e82bfb4d3fcf185fded4f8944f93e90eb/crates/rpc/rpc-builder/src/lib.rs#L1312-L1312



### Additional context

_No response_
