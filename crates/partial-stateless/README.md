# partial-stateless

Library for the **Partial Statelessness**: a protocol-level state cache that
models the state subset every network validator is assumed to hold, plus the
machinery to compute what a block's witness ("sidecar") must contain when some of
its accessed state is *not* in that cache.

The flat `NetworkStateCache` holds values and bytecode. The coordinated
`PartialTrieNodeCache` holds authenticated account paths and per-account storage
paths in a locally updated sparse trie.

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
the last *N* blocks. Its misses are exactly the values and bytecodes the sidecar
must supply; cache hits remain authoritative even if the sparse trie happens to
contain additional decoded nodes. The parent-state multiproof can also contain
structural targets needed for execution-diff paths or canonical branch/extension
transitions, so proof-target count is not always identical to value-cache miss
count. The sidecar carries no post-state proof: execution updates storage and
account paths locally, and the result is checked against the consensus block
root.

## Modules

| Module | Responsibility |
| --- | --- |
| [`accessed_state`](./src/accessed_state.rs) | `BlockAccessedState` — the read/write set captured from revm's `State` after executing a block |
| [`network_cache`](./src/network_cache.rs) | `NetworkStateCache` — insert/refresh/evict entries, `compute_miss`, footprint stats |
| [`trie_cache`](./src/trie_cache.rs) | persistent account/storage sparse trie, exclusion-aware path retention, shape metrics, and invariants |
| [`witness_check`](./src/witness_check.rs) | sidecar materialization, execution-diff application, and local post-state root calculation |
| [`policy`](./src/policy.rs) | `CachePolicy` trait + `LastNBlocksPolicy`; separate windows for accounts vs storage/code |
| [`witness`](./src/witness.rs) | turn a `MissResult` into `MultiProofTargets`, measure resulting proof size |
| [`sidecar`](./src/sidecar.rs) | serializable witness sidecar format + benchmark manifest |
| [`persistence`](./src/persistence.rs) | save/load the flat value cache; sparse-trie persistence is not implemented yet |
| [`bootstrap`](./src/bootstrap.rs) | verify a flat-cache snapshot plus authenticated account/storage paths and restore both cache layers |
| [`fixture`](./src/fixture.rs) | `AccessedStateFixture` — captured per-block access-sets for reproducible offline benchmarks |

## Persistence limitation

Only the flat value cache is written by the existing persistence helpers. The ExEx therefore
still cold-resets both caches on restart, reorg, and revert so a value is never treated as a cache
hit without its authenticated path. The library bootstrap API provides the portable alternative:
a `CacheSnapshotPackage` combines flat values with a state multiproof, verifies them against a
trusted cache anchor and canonical state root, and restores both cache layers atomically. Loading
that package during ExEx startup or transporting it over the peer protocol is not wired yet.

## Per-block trie synchronization

For each block, the value-cache miss is computed against the parent cache and the value-cache
transition applies its normal account and storage windows. The parent multiproof is then revealed
into a transactional sparse-trie clone, where the execution diff updates account values, storage
values, and account storage roots locally. The computed root must equal the block's consensus
state root. The already-computed value-cache membership then drives sparse-trie retention. A
failure rolls back the value-cache transition and discards the trie clone; the two caches advance
together only after the block checks succeed.

Retention keeps a complete lookup witness for every cached value. Existing accounts and nonzero
storage slots retain their inclusion paths. Nonexistent accounts and zero storage slots retain the
terminal branch, extension, or different leaf that proves exclusion. Unrelated decoded subtrees
are replaced with their authenticated hashes, so cache size follows the value-cache windows rather
than growing with every revealed proof. A per-account storage trie is discarded when its last
cached slot expires.

Only `NetworkStateCache` determines hits, misses, eviction, and cache anchors. Additional decoded
trie nodes never change the sidecar miss manifest or its anchors, and the sidecar carries no
post-state proof.

## Structural proof retries

Leaf deletion can collapse a branch or extension. A blinded sibling hash is sufficient to commit
to the parent state, but it does not reveal the sibling's node kind needed to construct the new
canonical shape. The sparse trie can therefore request additional parent-state structural nodes
while applying the execution diff.

The current ExEx provider exposes legacy leaf-key multiproofs rather than V2 structural targets.
`partial-stateless` converts those proofs to V2 locally. If a hashed extension child is missing,
the converter currently reports the first missing target; the ExEx adds it to the cumulative
legacy target set, regenerates the full proof, and retries from the parent trie snapshot. Deletion
targets discovered by one update phase are batched, but account conversion, storage conversion,
and the later update phases can still require separate rounds.

The intended follow-up is to collect all conversion gaps in one pass, request and merge only proof
deltas, and ultimately preserve the sparse-trie callback's `(key, min_len)` through a V2-capable
provider API. These retries fetch trie structure only: they do not turn a value-cache hit into a
miss or change either cache anchor.

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

Tests cover value-cache policies and rollback, sidecar commitments, persistent
account updates, storage-root propagation, transactional sparse-trie cloning,
retained-path invariants, integration flows, and reorg behavior.
