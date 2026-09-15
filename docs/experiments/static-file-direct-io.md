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

Jar data, offsets, and changeset-offset sidecars use a shared user-space block
cache with a 64 MiB total buffer budget across all files (plus bookkeeping and
in-flight reads). It has 16 independently locked LRU shards. Small reads fetch
4 KiB blocks, or the filesystem's required alignment if larger. Misses read
directly into aligned cache storage, avoiding a second temporary allocation and
copy. Entries survive individual cursor lifetimes through the shared reader.

Each opened reader gets a fresh identity and snapshots the file length. Reopening
after pruning, replacement, or recovery cannot reuse old cached contents. The
provider already replaces readers at these boundaries. Evicted readers' blocks
age out under the same global budget. Writes retain their existing buffering and
read-modify-write behavior.

The first measured version below instead used cursor-local 64 KiB read-ahead
windows that were discarded on cursor drop. The shared-cache revision is awaiting
its own benchmark results.

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

## Initial results (cursor-local read-ahead)

All three workflows completed successfully. This implementation substantially
regresses every measured workload, so it should remain an experiment.

Changes below are relative to upstream. The `±` values are the workflow's 95%
bootstrap confidence-interval half-widths, in percentage points. MGas/s means
million gas processed per second; RPC throughput is closed-loop requests/second.

| Workload / metric | Upstream | Direct I/O | Change (95% CI half-width) |
| --- | ---: | ---: | ---: |
| Engine mean block latency | 27.24 ms | 57.54 ms | +111.24% ±4.31 pp |
| Engine throughput | 1,230.57 MGas/s | 534.62 MGas/s | −56.56% ±0.89 pp |
| Depth-5 reorg mean block latency | 19.84 ms | 40.90 ms | +106.12% ±4.06 pp |
| Depth-5 reorg throughput | 1,569.16 MGas/s | 762.13 MGas/s | −51.43% ±1.14 pp |
| Historical trace mean latency | 34.57 ms | 466.92 ms | +1,250.75% ±33.51 pp |
| Historical trace P99 latency | 72.69 ms | 1,147.66 ms | +1,478.77% ±126.31 pp |
| Historical trace throughput | 56.93 requests/s | 3.62 requests/s | −93.65% ±0.49 pp |

The source data are `comment.md` and `summary.json` in each linked workflow's
`bench-results` artifact. Each comparison runs baseline and candidate on the same
runner and snapshot; absolute results across the different workloads should not
be compared as if they were the same test.

The historical corpus contains 20 consecutive blocks, 25,976,732–25,976,751,
using `debug_traceBlockByNumber` with `callTracer`. It has zero RPC errors on
both versions, all 80 candidate response comparisons match, and no records were
excluded for nondeterminism. CPU time per request rises from 30.54 ms to 82.50 ms
(+170.15%). Neither engine artifact contains a node-error report from the
workflow's panic/ERROR scan.

Startup is excluded from the replay measurements. On the engine runner,
read-ahead completes startup in 27–28 seconds across the six candidate runs,
versus 8–9 seconds for upstream. The snapshot requires history-index rebuilding;
these times are specific to that startup workload. The earlier unbuffered
implementation did not finish within the unchanged 300-second limit.

These measurements show that the existing higher-level caches do not hide the
cost of this implementation on these workloads. Aligned read amplification,
copying/allocation, and additional file operations are all possible contributors;
these runs do not isolate their individual effects. A different buffering or
file-layout strategy would require a new comparison.

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
