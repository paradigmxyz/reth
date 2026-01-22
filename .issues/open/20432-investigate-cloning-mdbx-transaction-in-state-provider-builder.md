---
title: Investigate cloning MDBX transaction in state provider builder
labels:
    - A-db
    - C-perf
assignees:
    - shekhirin
projects:
    - Reth Tracker
state: open
state_reason: null
parent: 20607
synced_at: 2026-01-21T11:32:16.014917Z
info:
    author: shekhirin
    created_at: 2025-12-16T19:54:42Z
    updated_at: 2026-01-07T13:19:42Z
---

## Summary

In `StateProviderBuilder::build()`, we currently create a new state provider by calling `state_by_block_hash`. MDBX supports cloning transactions via its API, which could potentially be leveraged here instead.

## Context

https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/mod.rs#L119-L122

```rust
pub fn build(&self) -> ProviderResult<StateProviderBox> {
    let mut provider = self.provider_factory.state_by_block_hash(self.historical)?;
    // ...
}
```

## Task

- Investigate whether using the MDBX transaction clone API would be beneficial here
- Determine if cloning a transaction is cheaper than creating a new one via `state_by_block_hash`
- If beneficial, implement the change using the appropriate MDBX API
