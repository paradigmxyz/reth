---
title: Remove `EthApiTypes::Error`
labels:
    - A-op-reth
    - A-rpc
    - A-sdk
    - C-debt
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.014288Z
info:
    author: emhane
    created_at: 2025-12-16T14:57:31Z
    updated_at: 2026-01-07T10:35:26Z
---

### Describe the feature

Remove `EthApiTypes::Error` in favour of implementing conversion to `EthApiError` via downcast, aka where `OpEthApiError` is defined add smthg like
```rust
impl From<OpEthApiError> for EthApiError {
        fn from(error: OpEthApiError) -> Self {
            EthApiError::Other(Box::new(error))
    }
}
```

### Additional context

This pattern can already be observed throughout the codebase as means to enable l2 errors to be used in l1 traits, and then recovered via downcasting, in order to be able to type safely match on l2 errors in l2 code. Originally we went with the approach to add an error as associated type to l1 traits, but then later decided that down casting is less complex (less loc and the syntax is OK readability).
