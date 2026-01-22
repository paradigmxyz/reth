---
title: Increase target-cpu in dist builds
labels:
    - C-perf
    - M-prevent-stale
    - P-high
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.98757Z
info:
    author: DaniPopes
    created_at: 2025-05-27T22:40:35Z
    updated_at: 2025-05-27T23:32:51Z
---

### Describe the feature

The default `-C target-cpu` for `x86_64-unknown-linux-gnu` is `x86-64`, which only has support for SSE2; we should investigate increasing this to, for example `haswell` for AVX2.

You can see what features a CPU has with:
```bash
rustc --target x86_64-unknown-linux-gnu -C target-cpu=haswell --print cfg
```

cc @Rjected 

### Additional context

_No response_
