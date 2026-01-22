---
title: Support tokio handle in CliRunner
labels:
    - A-cli
    - C-enhancement
assignees:
    - lean-apple
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.99606Z
info:
    author: mattsse
    created_at: 2025-08-20T15:34:11Z
    updated_at: 2025-08-20T15:42:43Z
---

### Describe the feature

We currently require a dedicated runtime for the entire node:

https://github.com/paradigmxyz/reth/blob/0f26562bb646b85f8e0b1a9c06184c88cce8e06d/crates/cli/runner/src/lib.rs#L22-L24

https://github.com/paradigmxyz/reth/blob/0f26562bb646b85f8e0b1a9c06184c88cce8e06d/crates/cli/runner/src/lib.rs#L144-L151

it would be nice to have if this would also work with just a tokio handle

for this, we can try something like

```
enum RuntimeOrHandle {
    Runtime(Runtime),
    Handle(Handle),
}
```

probably with some helper fn for block_on to make this work

https://github.com/paradigmxyz/reth/blob/0110fbe0a94858c04452b00c1edbb86d99df9965/crates/cli/runner/src/lib.rs#L57-L61

this needs some tests and examples because `handle.block_on` can easily panic if misused and needs:

https://github.com/paradigmxyz/reth/blob/0110fbe0a94858c04452b00c1edbb86d99df9965/crates/cli/runner/src/lib.rs#L117-L117



### Additional context

_No response_
