---
title: Introducing a stateless host crate
labels:
    - C-enhancement
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.994907Z
info:
    author: kevaundray
    created_at: 2025-08-12T21:39:07Z
    updated_at: 2025-09-03T08:55:44Z
---

### Describe the feature

Currently we have `crates/stateless` for the guest program -- This is a suggestion to add a host crate:

```
- crates/stateless/guest
- crates/stateless/host
```

The code that is currently on master would be placed into stateless/guest and this would be the code that gets compiled to riscv/proven.
The code in stateless/host would be reth specific code that the host/prover would utilize in order to prepare inputs for the guest. 

One example of this is where the guest will be given public keys along with the block, and the host would need to recover the public keys from transactions in order to pass them to the guest.

### Additional context

_No response_
