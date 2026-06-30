# `partial-stateless` binaries

Standalone command-line tools that operate on the artifacts produced by the
[`partial-stateless-exex`](../../../partial-stateless-exex). Each is a thin CLI
over the library in [`../`](../) — none of them needs a running node, so they are
cheap to run in CI or on a laptop with just the files the ExEx wrote to disk.

| Binary | Input | Answers |
| --- | --- | --- |
| [`sidecar_verifier`](./sidecar_verifier.rs) | `sidecar/*.bin` (+ optional reference witnesses) | Is the witness sidecar cryptographically anchored to the parent state root, and how complete is it? |
| [`cache_window_bench`](./cache_window_bench.rs) | `fixtures/accessed/*.bin` | How does the cache window trade off against hit ratio, and which state category is the hot-spot? |

Run any of them with `cargo run -p partial-stateless --bin <name> -- <args>`.

---

## `sidecar_verifier`

Verifies the witness sidecars the ExEx emits, without trusting the producer or
the transport. Two independent checks:

1. **Crypto integrity** (default) — every missed account proof verifies against
   `parent_state_root`, every missed storage proof against its account
   `storageRoot`, and every supplied bytecode preimage hashes to a declared
   missed code hash. Proves the material that *is* present is anchored to the
   parent state root. Does **not** prove completeness.
2. **Coverage** (`--coverage`) — compares the sidecar against a reference
   `debug_executionWitness` (ground truth from a full node) for the same block,
   reporting how much of the state actually needed to re-execute is present.

```bash
# Crypto integrity over a directory of sidecars
cargo run -p partial-stateless --bin sidecar_verifier -- ./sidecar

# Coverage against reference witnesses (one witness_<N>.json per block N)
cargo run -p partial-stateless --bin sidecar_verifier -- \
    --coverage --witness-dir ./witnesses ./sidecar
```

Arguments:

| Flag | Meaning |
| --- | --- |
| `<path-or-dir> ...` | one or more sidecar `.bin` files or directories of them |
| `--coverage` | enable coverage mode (requires `--witness-dir`) |
| `--witness-dir <dir>` | dir of `witness_<block>.json` reference witnesses |

Neither mode re-executes the block; full re-execution (`ACCEPT_BLOCK`) is a
follow-up. Exit code is non-zero if any sidecar fails its check.

---

## `cache_window_bench`

Sweeps the cache eviction window against hit ratio over a **fixed, captured**
dataset, fully offline (no node, no EVM, no Merkle proofs). It replays the
per-block `BlockAccessedState` fixtures through a fresh `NetworkStateCache` for
every `(account_window, storage_window)` in a grid and reports hit ratio
(overall + per-category), cache footprint, and a hot-spot breakdown.

First capture a dataset once, by running the ExEx with `PS_CAPTURE_DIR` set (see
the [ExEx README](../../../partial-stateless-exex/README.md)). Then:

```bash
# Default 6×6 grid over ./fixtures/accessed
cargo run -p partial-stateless --bin cache_window_bench -- --fixtures ./fixtures/accessed

# Custom grid + baseline for the hot-spot breakdown
cargo run -p partial-stateless --bin cache_window_bench -- \
    --fixtures ./fixtures/accessed \
    --account-windows 30,60,90,128 --storage-windows 8,30,60 \
    --baseline 60,60 --out bench.csv
```

Arguments:

| Flag | Default | Meaning |
| --- | --- | --- |
| `--fixtures <dir>` | `./fixtures/accessed` | directory of `accessed_*.bin` fixtures |
| `--account-windows a,b,c` | `8,16,30,60,90,128` | account `LastN` windows to sweep |
| `--storage-windows a,b,c` | `8,16,30,60,90,128` | storage/code `LastN` windows to sweep |
| `--warmup <N>` | largest window in the grid | blocks to warm before scoring |
| `--baseline aw,sw` | `60,60` | config used for the hot-spot breakdown |
| `--out <path>` | `./cache_window_bench.csv` | CSV output |

**Fairness note:** the cache is warmed over the first `--warmup` blocks (still
updated, but not scored) so every config is measured over the *same* fully-warmed
block range. Default warmup = the largest window, so even the widest window is
warm before scoring begins.

`LastNBlocksPolicy` is a *time* window (blocks), not a size cap — so "cache size"
is an emergent output reported per config, not an input you pin.

To smoke-test without a node, generate synthetic fixtures with the example:

```bash
cargo run -p partial-stateless --example gen_synth_fixtures -- /tmp/fix 300
cargo run -p partial-stateless --bin cache_window_bench -- --fixtures /tmp/fix
```
