---
title: Add prometheus metrics scraper to reth-bench
labels:
    - A-cli
    - C-benchmark
    - C-enhancement
    - C-perf
    - M-prevent-stale
projects:
    - Reth Tracker
state: open
state_reason: REOPENED
synced_at: 2026-01-21T11:32:15.98144Z
info:
    author: Rjected
    created_at: 2025-02-05T18:28:46Z
    updated_at: 2025-09-26T15:35:07Z
---

Right now `reth-bench` outputs metrics like gas per second based on total newPayload + forkchoiceUpdated latency:
https://github.com/paradigmxyz/reth/blob/abe946e2452651e41f85f8d89cd5d5196c156c72/bin/reth-bench/src/bench/new_payload_fcu.rs#L119

This is slightly confusing, because it is measuring gas per time spent on more than just execution. Instead, we could actually show much more granular metrics if the node has a `--metrics` endpoint configured.

We could then scrape these metrics after successful nP / FCU for the block:
https://github.com/paradigmxyz/reth/blob/main/crates/engine/tree/src/tree/metrics.rs#L60-L80

https://github.com/paradigmxyz/reth/blob/abe946e2452651e41f85f8d89cd5d5196c156c72/crates/engine/tree/src/tree/metrics.rs#L8-L19

This would allow us to scrape, and record, the raw execution and state root durations for the node.
It's important that we scrape and parse the metrics endpoint ourself, rather than rely on a prometheus instance, because then we can get block-level granularity for these metrics

To do this, we need:
* A command line flag indicating that `reth-bench` should try to fetch metrics
* Methods that can scrape and parse the scraped metrics


This [pr](https://github.com/paradigmxyz/reth/pull/14639) is just configuring the URL, the full mvp should:
- have url configuration, prometheus client creation
 scrape after every block
- record results somewhere in a structured way, similar way to the reth-bench csv (maybe key, value, some other info)
