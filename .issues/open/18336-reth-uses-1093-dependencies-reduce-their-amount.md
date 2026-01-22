---
title: 'reth uses 1093 dependencies: reduce their amount'
labels:
    - C-security
    - M-prevent-stale
    - S-feedback-wanted
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.998115Z
info:
    author: paulmillr
    created_at: 2025-09-09T01:09:10Z
    updated_at: 2025-10-29T21:53:48Z
---

I've noticed reth is using 1093 deps:

    cat Cargo.lock | grep '[[package]]' | wc -l

Since every dependency is a potential security vulnerability (think supply chain attacks), it would be great to reduce their amount.

The fact most deps aren't controlled by reth team is also not ideal. If it was e.g. 80% of controlled deps, it would've been better.

Do you think there is a way to improve the situation?


### Code of Conduct

- [x] I agree to follow the Code of Conduct
