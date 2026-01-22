---
title: Reliable eth_getProof support for recent state (e.g. last 7 days)
labels:
    - A-engine
    - A-op-reth
    - A-trie
    - C-enhancement
    - C-perf
    - S-needs-design
assignees:
    - mediocregopher
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:15.996959Z
info:
    author: jenpaff
    created_at: 2025-08-26T14:13:04Z
    updated_at: 2025-12-10T12:11:12Z
---

### Describe the feature

**Problem Statement**

Applications on Optimism and other rollups (e.g. ENS, Base infra) rely on fast and reliable eth_getProof queries within the 7-day challenge window. At the moment, this is a blocker for Reth adoption.

While Erigon has recently demonstrated an archive format that compresses all historical proofs into ~5 TB ([tweet](https://x.com/GiulioRebuffo/status/1958152689612718355)
). Most use cases do not require full history, a rolling recent window is sufficient for 95% of needs.

**Desired outcome**
Reth should provide a way to serve eth_getProof efficiently over a certain time (e.g. 7 days). This likely involves persisting and exposing state trie data in a way that can be queried quickly, but the exact design is open (engine integration, ExEx hooks, sidecar, or other approaches).

**Note that there's a couple of ways to implement this, this task should be kicked-off with a design doc.** 

### Additional context
Related work: [#17967](https://github.com/paradigmxyz/reth/issues/17967): Support accessing proof data within an ExEx.
