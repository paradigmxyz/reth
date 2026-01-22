---
title: Create performance specific grafana dashboard
labels:
    - A-engine
    - A-observability
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-needs-triage
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.975875Z
info:
    author: Rjected
    created_at: 2024-09-16T17:11:45Z
    updated_at: 2025-07-08T19:38:59Z
---

We now have many metrics w.r.t. latency, throughput, system specs, memory allocated, etc. These metrics are important for performance but mixed into the rest of the panels. There are also many metrics we could collect that are perf-related, that we should not bloat the regular dashboard with.

We should then create a separate dashboard that focuses on performance metrics.
