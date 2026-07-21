# partial-stateless-exex

A reth [Execution Extension (ExEx)](../exex/exex) that drives the
[`partial-stateless`](../partial-stateless) library from a live node. It maintains
the network-level state cache as the chain advances and, per block, measures the
witness ("sidecar") a partially-stateless validator would need.

The binary is `reth-partial-stateless` — a full Ethereum node with the ExEx
installed.

## What it does per committed block

1. Re-executes the block against its parent state (`history_by_block_number`) and
   captures the `BlockAccessedState` (accounts, storage, bytecodes touched).
2. Computes the **cache miss** *before* updating the cache — this is what a
   validator joining at this block would have to be sent.
3. Applies the tentative `NetworkStateCache` transition (including `LastNBlocksPolicy` eviction),
   retaining a rollback record until the sparse-trie transition and sidecar checks succeed.
4. Computes the parent-state Merkle multiproof for cache misses plus uncovered execution-diff
   paths. If a canonical deletion or legacy-to-V2 extension conversion needs a blinded
   sibling/child, it adds a structural target and regenerates the cumulative legacy multiproof.
   Structural targets do not change the cache-miss manifest. It writes a **witness sidecar** + a
   JSON benchmark **manifest** to `./sidecar/`.
5. *(optional)* Runs the **provider-assisted sidecar preflight** — re-executes
   the block through a cache+witness-backed provider and checks the miss set plus
   cache-anchor transition.
6. *(optional)* Computes the **full-witness baseline** — a second multiproof over
   *all* accessed state, ignoring the cache — to report the reduction ratio.
7. Logs accessed/missed counts, miss ratio, witness size, and cache footprint.

The parent-state multiproof is revealed into a cloned local sparse trie. Storage and account
changes are applied locally and the computed post-state root is checked against the block header.
The tentative flat-cache membership produced in step 3 is then mirrored into the sparse trie:
inclusion paths are retained for existing values, while zero and nonexistent values retain the
terminal exclusion node. Unrelated decoded subtrees are blinded and an account's storage trie is
removed after its final cached slot expires. On failure, the value transition is rolled back and
the cloned trie is discarded. The sidecar carries no post-state proof.

The flat `NetworkStateCache` alone decides hits, misses, eviction, and cache anchors. Sparse-trie
shape is local validation state: additional revealed nodes do not change the sidecar miss manifest
or either cache anchor.

Sparse-trie snapshots currently have no branch-aware undo representation. On
`ChainReorged` and `ChainReverted`, both flat and trie caches are cold-reset so a
flat value cannot outlive its authenticated path. A builder can initialize from
the full provider while processing the new branch. The library can verify and
restore a synchronized joint cache snapshot, but this ExEx does not yet load that
package during startup, so a sidecar-only verifier cannot recover from the reset.

## Run

```bash
cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
```

The flat cache is persisted to `<datadir>/partial_stateless_cache.bin`, but the
matching sparse-trie snapshot is not yet persisted. A non-empty persisted value
cache is therefore cold-reset on restart.

### Configuration

The cache windows are set in `CacheConfig` ([main.rs](./src/main.rs)) — default
`account_window = 60`, `storage_window = 30` blocks. Adjust there and rebuild.
(Use [`cache_window_bench`](../partial-stateless/src/bin/README.md) to pick good
values offline before committing to them.)

Optional diagnostic/benchmark features are off by default and enabled per run via environment
variables, so the core sidecar generation path stays lean:

| Env var | Effect |
| --- | --- |
| `PS_SIDECAR_ROLE=builder\|builder-verifier\|verifier` | choose whether this ExEx writes sidecars, writes and preflights them, or consumes existing sidecars as a live verifier (default: `builder`) |
| `PS_SIDECAR_DIR=<dir>` | write sidecars in `<dir>` (default: `./sidecar`) |
| `PS_SIDECAR_VERIFIER_WAIT_MS=<ms>` | in `verifier` mode, wait up to this long for the block sidecar file to appear (default: `2000`) |
| `PS_CAPTURE_DIR=<dir>` | dump each block's `BlockAccessedState` fixture to `<dir>` (see below) |
| `PS_WITNESS_BASELINE=1` | also compute the full-witness baseline + reduction ratio (an extra, larger multiproof per block) |
| `PS_RESOURCE_METRICS=1` | capture per-thread CPU time + page faults around the partial multiproof (`cpu_time_ms`, `major_page_faults`, `minor_page_faults`) to separate compute-bound from disk-I/O-bound blocks |
| `PS_SIDECAR_PREFLIGHT=1` | run provider-assisted validator preflight for each sidecar (an extra re-execution per block) |
| `PS_TRIE_CACHE_DIAGNOSTICS=1` | validate retained account/storage paths and log trie shape, memory, and transition timings |

`PS_SIDECAR_ROLE=builder-verifier` is a single-process test mode: it keeps the
normal builder output path, but forces the same provider-assisted client preflight
before publishing each sidecar. Use this mode to observe cache-miss-only,
witness-integrity, state-root, and next-cache-anchor failures while the builder is
running.

`PS_SIDECAR_ROLE=verifier` is the live verifier mode. It does not build or publish
sidecars. For each canonical block it reads
`$PS_SIDECAR_DIR/block_<N>_<hash>.bin`, verifies it against the local previous
cache, re-executes with cache hits plus sidecar miss witnesses, and advances the
local cache only after verification succeeds. The verifier must start with a
cache synchronized to the parent block; the sidecar file alone is not enough to
reconstruct that previous cache. Because sparse-trie snapshots are not persisted,
the current binary cold-resets a persisted flat cache at startup; ordinary
mid-chain verifier restart/cold-start is therefore not implemented yet.

`PS_SIDECAR_PREFLIGHT` gates the validator-like self-check. When enabled, sidecar generation fails fast if the cache+witness-backed re-execution, expected miss set, or next cache anchor check fails. When unset, the sidecar still carries `prev_cache_anchor`, `next_cache_anchor`, and `witness_commitment`, but this ExEx does not spend the extra execution work to preflight them. The manifest records this as `provider_assisted_preflight: false`.

Preflight re-executes from cache hits plus sidecar misses, applies the execution
diff to a cloned local sparse trie, checks that root against the consensus block
root, and then cross-checks it with the full provider. It also verifies the miss
set and value-cache next anchor.

The manifest and verifier logs expose
`partial_state_trustless_verification_ready`. The readiness calculation includes
miss paths from the sidecar and cache-hit paths retained by the local sparse trie.

When the first processed block is not synchronized to the cache parent, the
builder obtains a local-only proof for the union of captured access paths and
execution-diff paths. It uses that proof to initialize both local caches and does
not publish a cache-coherent sidecar for that block. This is local ExEx startup,
not a protocol bootstrap mechanism for a new stateless node. Cold-EOA mempool
admission and new-node cache bootstrap remain out of scope.

When `PS_WITNESS_BASELINE` is unset, the manifest's `full_sidecar_baseline_stats`
and `reduction` are `null` and no baseline multiproof is computed. A baseline
failure is non-fatal — it never blocks the real (partial) sidecar.

When `PS_RESOURCE_METRICS` is unset, the partial stats' `cpu_time_ms`,
`major_page_faults`, and `minor_page_faults` are `null` and no `getrusage`
syscalls are made. The metrics are Linux-only (`RUSAGE_THREAD`); on other
platforms they log zeros. If comparing against the baseline, note that
`PS_WITNESS_BASELINE` runs first and can warm the OS page cache, deflating the
partial proof's page-fault counts.

### Structural proof retries

Some post-state deletions require the decoded type of a blinded sibling or extension child before
the sparse trie can form the canonical collapsed branch. This is a structural parent-state proof
requirement, not a value-cache miss. The current provider interface accepts legacy leaf-key
targets, while the sparse-trie update API describes structural requests as `(key, min_len)` V2
targets. The adapter currently zero-pads the requested prefix, drops `min_len`, and converts the
legacy proof to V2 locally.

The current retry loop adds newly discovered targets to the cumulative target set and regenerates
the entire legacy multiproof. Legacy-to-V2 conversion also stops at the first missing hashed
extension child, although deletion targets within one update phase are batched. Thus a block can
show several one-target retries followed by a larger deletion batch. Planned improvements are to:

1. collect every account/storage conversion gap before retrying;
2. request only new proof targets and merge each delta into the accumulated proof; and
3. expose V2 proof generation through the provider so `min_len` is preserved.

Until those changes land, `partial_sidecar_stats.computation_time_ms` covers cumulative wall time
from the first legacy proof request through the final successful proof request, including time
spent in failed local trie attempts between requests. The final sidecar still contains one
self-contained, parent-root-verified proof.

### Trie-shape diagnostics

`PS_TRIE_CACHE_DIAGNOSTICS=1` performs an O(retained paths) scan after each
builder transition. It checks exact flat/trie membership, a complete inclusion or
exclusion witness for every retained account and storage path (including zero and
nonexistent values), and equality of the recomputed sparse root and recorded
post-state root. Successful blocks log clone, update, retention, and validation
timings; memory; decoded account/storage node counts; and hashed-key prefix
coverage at depths zero through five.

The clone and local-root timing fields describe the final successful retry only; discarded clones
and failed local transitions are included in the cumulative proof computation time instead.
Retention is normal per-block cache work. Full validation is diagnostic-only and is skipped when
`PS_TRIE_CACHE_DIAGNOSTICS` is unset.

Combine diagnostics with `PS_SIDECAR_PREFLIGHT=1` for a bounded correctness run.
Do not interpret prefix coverage as a literal MPT node count: Patricia extensions
compress nibble levels.

### Capturing a benchmark dataset

Set `PS_CAPTURE_DIR` to dump each block's `BlockAccessedState` as a fixture. This
reuses the exact execution path the live system uses, so the dataset is faithful —
and once captured, the offline `cache_window_bench` needs no node at all.

```bash
PS_CAPTURE_DIR=./fixtures/accessed \
    cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
# let it run until ~300 accessed_*.bin files exist, then stop
```

Re-injecting *raw blocks* would not be reproducible — re-execution needs the parent
historical state present in the node DB at that exact height. The accessed-state
snapshot is the portable, self-contained artifact.

## Outputs

| Path | Contents |
| --- | --- |
| `<datadir>/partial_stateless_cache.bin` | persisted flat cache; cold-reset on restart until trie snapshots are persisted |
| `./sidecar/block_<N>_<hash>.bin` | witness sidecar (or `$PS_SIDECAR_DIR/block_<N>_<hash>.bin`) |
| `./sidecar/block_<N>_<hash>.manifest.json` | per-block benchmark manifest |
| `$PS_CAPTURE_DIR/accessed_<N>.bin` | captured fixture (when capture is enabled) |
