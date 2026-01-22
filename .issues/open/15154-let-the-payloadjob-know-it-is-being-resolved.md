---
title: Let the payloadjob know it is being resolved
labels:
    - C-enhancement
    - D-good-first-issue
assignees:
    - RomanHodulak
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.983233Z
info:
    author: mattsse
    created_at: 2025-03-19T18:32:59Z
    updated_at: 2025-12-03T20:37:38Z
---

### Describe the feature

currently if a payloadjob is being resolved we start racing it against an empty block:

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/payload/basic/src/lib.rs#L481-L485

and the default eth builder just tries to max fill the block:

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/ethereum/payload/src/lib.rs#L185-L185

so in case the payload is resolved immediately, we might end up with an empty block if we race it against an empty block.

instead we want to let the job know that it is being resolved right now

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/ethereum/payload/src/lib.rs#L86-L98

for this we can introduce another bool marker, like:

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/payload/basic/src/lib.rs#L786-L787

which is invoked by the job here:

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/payload/basic/src/lib.rs#L355-L357

which we then can trigger when the payload is being resolved:

https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/payload/basic/src/lib.rs#L458-L458


## TODO
* introduce a bool toggle for the `BasicPayloadJob` that is passed as part of `BuildArguments` and a clone kept in `BasicPayloadJob`
* when the `BasicPayloadJob` is being resolved, it should fire the toggle
* and the builder tx loop should break if the toggle is fired, similar to: https://github.com/paradigmxyz/reth/blob/9f6fec213c839db28c4001d723c409c4495ab91f/crates/ethereum/payload/src/lib.rs#L198-L201



### Additional context

_No response_
