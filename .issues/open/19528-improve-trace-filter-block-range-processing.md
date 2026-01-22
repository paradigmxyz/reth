---
title: Improve trace_filter block range processing
labels:
    - A-rpc
    - C-enhancement
    - C-perf
assignees:
    - frankudoags
projects:
    - Reth Tracker
state: open
state_reason: null
synced_at: 2026-01-21T11:32:16.003168Z
info:
    author: mattsse
    created_at: 2025-11-05T19:52:16Z
    updated_at: 2025-11-09T11:38:13Z
---

### Describe the feature

currently we do this sequentially:

https://github.com/paradigmxyz/reth/blob/ba8be3fb64d51d712f62ce4ed26d46842bd155e3/crates/rpc/rpc/src/trace.rs#L398-L415

this does

1. spawn a blocking task to fetch the block range
2. the spawn tracing future per block


this can be significantly improved by doing things concurrently and also leveraging the rpc cache, we already have a few optional helpers like:

https://github.com/paradigmxyz/reth/blob/ba8be3fb64d51d712f62ce4ed26d46842bd155e3/crates/rpc/rpc-eth-types/src/cache/mod.rs#L206-L207

that we could use here as well if the requested range is close to the head block (within ~1000 blocks)

## TODO
* convert this function into something that yields fetched blocks to something that can  process them concurrently 

```rust
  let mut tracing_futures_ordered =...;
  let mut block_stream_ordered =...;
  tokio::select! {
       block_stream_ordered.next() => tracing_futures_ordered.push(...),
       tracing_futures_ordered.next() => {process trace, maybe exit}
   } 

```



### Additional context

_No response_
