---
title: Fine-tune async task scheduling
labels:
    - A-rpc
    - C-enhancement
    - C-perf
    - M-prevent-stale
    - S-needs-investigation
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.993601Z
info:
    author: hai-rise
    created_at: 2025-07-25T05:43:23Z
    updated_at: 2025-08-24T09:48:18Z
---

(Correct me if I'm wrong, but) Currently, Reth spawns most, if not all, asynchronous tasks onto the same Tokio runtime. This is fine most of the time, but not ideal for latency-sensitive nodes. For instance, we noticed that our shred channel for RPC-serving (lots of competing async tasks) nodes could buffer significantly, due to the processing task not being timely polled. Spawning shred tasks onto a dedicated Tokio runtime indeed solved the problem.

Nevertheless, the flaky nature of RPC latency is still there. This may be fine for a historical `eth_getBlockbyNumber` or `eth_getLogs`, but not ideal for aggressive pending/latest states. For instance, adding a single transaction to the mempool (`eth_sendRawTransaction`) can fluctuate between instantly and ...30ms! This is also under only 3xx TPS. Ideally, this should be 0~3ms even under 1000s of TPS.

<img width="1192" height="598" alt="Image" src="https://github.com/user-attachments/assets/2ca41bc8-bcf1-4c28-aeba-caa689f91e47" />

For a solution, we should have two Tokio runtimes, so we can spawn latency-sensitive tasks (shred handling, `eth_subscribe`, `eth_sendRawTransactionSync`, etc.) on a dedicated one, and everything else on another. Can even spawn another (with only 1~2 worker threads and high intervals) for very latency-insensitive tasks like purging caches.

This sounds like a long journey, as we'll need to categorise tasks by latency-sensitivity, and work with dependencies like `jsonrpsee` for selecting runtime at the RPC method-level, etc. Maybe we can keep this issue for the overall context, while opening smaller ones for each step 🤔.
