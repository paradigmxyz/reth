# partial-stateless

Library for the **Partial Statelessness**: a protocol-level state cache that
models the state subset every network validator is assumed to hold, plus the
machinery to compute what a block's witness ("sidecar") must contain when some of
its accessed state is *not* in that cache.

This crate is node-independent — it operates on plain state access-sets, not on a
reth database. The [`partial-stateless-exex`](../partial-stateless-exex) drives it
from a live node; the [binaries](./src/bin/) and [example](./examples/) exercise
it offline.

## Mental model

```
                 (per block, from EVM execution)
  BlockAccessedState  ──compute_miss──▶  MissResult  ──▶  witness targets ──▶ sidecar
         │                  ▲
         └──on_block_executed──┘
              NetworkStateCache  (LastNBlocksPolicy: accounts vs storage/code)
```

A block's execution touches a set of accounts, storage slots, and bytecodes
(`BlockAccessedState`). The `NetworkStateCache` holds whatever was accessed within
the last *N* blocks. State the cache is missing is exactly what must be shipped as
a Merkle-proof witness — so **cache miss ratio = witness requirement**.

## Modules

| Module | Responsibility |
| --- | --- |
| [`accessed_state`](./src/accessed_state.rs) | `BlockAccessedState` — the read/write set captured from revm's `State` after executing a block |
| [`network_cache`](./src/network_cache.rs) | `NetworkStateCache` — insert/refresh/evict entries, `compute_miss`, footprint stats |
| [`policy`](./src/policy.rs) | `CachePolicy` trait + `LastNBlocksPolicy`; separate windows for accounts vs storage/code |
| [`witness`](./src/witness.rs) | turn a `MissResult` into `MultiProofTargets`, measure resulting proof size |
| [`sidecar`](./src/sidecar.rs) | serializable witness sidecar format + benchmark manifest |
| [`persistence`](./src/persistence.rs) | save/load the warm cache to disk (survives node restarts) |
| [`fixture`](./src/fixture.rs) | `AccessedStateFixture` — captured per-block access-sets for reproducible offline benchmarks |

## Binaries & examples

- [`src/bin/`](./src/bin/) — `sidecar_verifier` (trustless witness verification) and
  `cache_window_bench` (offline cache-window vs hit-ratio sweep). See the
  [bin README](./src/bin/README.md).
- [`examples/gen_synth_fixtures.rs`](./examples/gen_synth_fixtures.rs) — generate
  synthetic fixtures to smoke-test the benchmark without a node.

## Test

```bash
cargo test -p partial-stateless
```

Unit tests cover eviction windows, miss computation, refresh-extends-lifetime, and
fixture round-trips; [`tests/integration_test.rs`](./tests/integration_test.rs)
covers the end-to-end cache flow.
