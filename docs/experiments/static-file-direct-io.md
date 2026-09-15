# Static-file direct I/O experiment

## Implementation

Static-file data, column offsets, changeset-offset sidecars, and jar configuration
use cache-bypassing file I/O. Linux enables `O_DIRECT`, querying `STATX_DIOALIGN`
when available and using 4 KiB alignment on older kernels. Apple platforms enable
`F_NOCACHE`. Platforms without these mechanisms, and filesystems that explicitly
reject them, fall back to positional file I/O. Static-file readers no longer mmap
files on any platform.

The on-disk format is unchanged. Reads round requests out to the required alignment
and copy the requested bytes into cursor-owned buffers. Writes preserve partial
edge blocks with read-modify-write, then truncate padding to the logical file size.
The existing user-space write buffering, config commit boundary, and fsync ordering
are retained. Direct I/O does not itself provide fsync durability.

Cursor-local read-ahead windows amortize sequential scans: 64 KiB each for data
and offsets, growing for an individual oversized value. They are released with
the cursor; inactive jars retain no cached file contents. Small lookups still
require device I/O, and partial writes require read-modify-write. Changeset-offset
sidecars retain buffered writes and read range records in batches.

The experiment bypasses the kernel cache while retaining higher-level caches
and the existing file format. It does not change MDBX or RocksDB I/O.

## Baseline and benchmark method

The branch was started from upstream `main` after fetching on 2026-09-15:
`5d0ea55c5246b883fcefda2ebe9ba03a240d4d03`.
The measured candidate with cursor read-ahead is
`10f5fe6755fd8819f044c1c9621991afdb631dee`.

The Engine API runs use Reth's `bench.yml` workflow, 500 blocks, six run pairs,
and the default 125-block warmup. Each workflow restores the snapshot between
baseline and candidate runs and uses the workflow's alternating run order and
statistical summary. Historical block tracing uses 20 source blocks, four run
pairs, a 30-second warmup, 60 seconds at two requests per second, and two
closed-loop passes with concurrency two. Both binaries are pinned to commit IDs.

| Workload | Run |
| --- | --- |
| Engine API replay | [35003191876](https://github.com/paradigmxyz/reth/actions/runs/35003191876) |
| Engine API replay with depth-5 reorgs | [35003194833](https://github.com/paradigmxyz/reth/actions/runs/35003194833) |
| Historical `debug_traceBlockByNumber` | [35003197593](https://github.com/paradigmxyz/reth/actions/runs/35003197593) |

## Earlier experiment findings

The initial implementation issued separate direct reads for each offset and
column value. Both [engine replay](https://github.com/paradigmxyz/reth/actions/runs/35000271603)
and [reorg replay](https://github.com/paradigmxyz/reth/actions/runs/35000516440)
hit the workflow's 300-second startup limit while rebuilding storage history,
before any replay measurements. These failures motivated cursor-local read-ahead;
the timeout remains unchanged. A [sidecar-batching-only run](https://github.com/paradigmxyz/reth/actions/runs/35001832629)
also hit the same startup limit.

The first [trace attempt](https://github.com/paradigmxyz/reth/actions/runs/35000785661)
also failed during preparation. Corpus extraction probed historical blocks as
soon as RPC responded while history indexing was still running. The branch now
waits for `eth_syncing == false`, matching the benchmark runner's existing readiness
condition and 300-second bound. An [upstream-only diagnostic](https://github.com/paradigmxyz/reth/actions/runs/35001749025)
completed a one-block trace; that control is not a direct-I/O performance result.

## Validation

- Nightly rustfmt and all-target/all-feature storage clippy.
- Unit coverage for actual Linux `O_DIRECT`, unaligned reads/writes, edge-block
  preservation, truncation, sparse extension, EOF, and mixed write sequences.
- Cursor tests cover sequential scans, random seeks, column masks, compression,
  empty values, oversized reads, and read-ahead boundaries.
- Existing provider tests cover pruning and interrupted-commit recovery; both
  large RocksDB history-healing tests pass locally.
- `reth-fs-util` also compiles for `wasm32-wasip1`; private-item docs build.
- Dependency checks: `zepter` and `make lint-toml` pass.
- [Linux validation of the measured candidate](https://github.com/paradigmxyz/reth/actions/runs/35003175567)
  passed all 28 direct-file/storage tests, workspace clippy, and 3,462 workspace
  unit tests (one retried; seven skipped by the suite configuration).
  The workspace test selection excludes `ef-tests`, integration-test binaries,
  and `e2e_testsuite`.

## Interpretation limits

These workloads measure node performance, including persistence, reorgs, and
historical RPC reads. They do not directly quantify static-file page-cache
residency. Throughput alone cannot establish a memory saving or predict
performance for a different historical range or access pattern. Linux `O_DIRECT`
and Apple's `F_NOCACHE` also have different operating-system semantics.
