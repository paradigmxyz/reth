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
3. Updates the `NetworkStateCache` (applies the `LastNBlocksPolicy` eviction).
4. Computes the actual Merkle multiproof for the missed state and writes a
   **witness sidecar** + a JSON benchmark **manifest** to `./sidecar/`.
5. *(optional)* Computes the **full-witness baseline** — a second multiproof over
   *all* accessed state, ignoring the cache — to report the reduction ratio.
6. Logs accessed/missed counts, miss ratio, witness size, and cache footprint.

On `ChainReorged` the new chain is replayed into the cache; `ChainReverted` is
logged but not rolled back (PoC limitation).

## Run

```bash
cargo run -p partial-stateless-exex -- node --chain mainnet --datadir /path/to/data
```

The warm cache is persisted to `<datadir>/partial_stateless_cache.bin` and reloaded
on restart (with a gap-tolerance check), so the cache survives node restarts and
short downtime without going cold.

### Configuration

The cache windows are set in `CacheConfig` ([main.rs](./src/main.rs)) — default
`account_window = 60`, `storage_window = 30` blocks. Adjust there and rebuild.
(Use [`cache_window_bench`](../partial-stateless/src/bin/README.md) to pick good
values offline before committing to them.)

Two benchmark-only features are off by default and enabled per run via environment
variables, so the core path stays lean:

| Env var | Effect |
| --- | --- |
| `PS_CAPTURE_DIR=<dir>` | dump each block's `BlockAccessedState` fixture to `<dir>` (see below) |
| `PS_WITNESS_BASELINE=1` | also compute the full-witness baseline + reduction ratio (an extra, larger multiproof per block) |

When `PS_WITNESS_BASELINE` is unset, the manifest's `full_sidecar_baseline_stats`
and `reduction` are `null` and no baseline multiproof is computed. A baseline
failure is non-fatal — it never blocks the real (partial) sidecar.

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
| `<datadir>/partial_stateless_cache.bin` | persisted warm cache |
| `./sidecar/block_<N>_<hash>.bin` | witness sidecar (verify with `sidecar_verifier`) |
| `./sidecar/block_<N>_<hash>.manifest.json` | per-block benchmark manifest |
| `$PS_CAPTURE_DIR/accessed_<N>.bin` | captured fixture (when capture is enabled) |
