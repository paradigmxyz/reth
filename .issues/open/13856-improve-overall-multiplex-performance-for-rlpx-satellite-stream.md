---
title: Improve overall multiplex performance for rlpx satellite stream
labels:
    - A-networking
    - C-enhancement
    - C-perf
    - D-good-first-issue
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.980466Z
info:
    author: mattsse
    created_at: 2025-01-18T12:10:31Z
    updated_at: 2026-01-05T11:05:53Z
---

### Describe the feature

the sink impl for 

https://github.com/paradigmxyz/reth/blob/bcf4f1bae361846a3b9ff63ca63affeca311055e/crates/net/eth-wire/src/multiplex.rs#L579-L585

has a smol flaw that could potentially starve rlpx satellite impls.

because this will always bypass the satellite messages that are currently buffered

https://github.com/paradigmxyz/reth/blob/bcf4f1bae361846a3b9ff63ca63affeca311055e/crates/net/eth-wire/src/multiplex.rs#L587-L600

to fix this we should move the sink logic embedded in the stream impl into the sink impl

https://github.com/paradigmxyz/reth/blob/bcf4f1bae361846a3b9ff63ca63affeca311055e/crates/net/eth-wire/src/multiplex.rs#L469-L479

this way poll_ready should always drain the output buffer of satellite msgs first treating all messages fcfs

FYI @0xvanbeethoven @Will-Smith11

### Additional context

_No response_
