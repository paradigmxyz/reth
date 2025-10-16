# Static File DB Write/Read Optimisation Tickets

This document breaks the identified bottlenecks in the static-file pipeline into actionable GitHub issues. Each issue includes context, proposed approach, validation steps, and references so it can be filed directly.

## Issue 1: Reuse MDBX cursors in static-file segment writers

- **Area**: `crates/static-file/static-file/src/segments/{transactions,receipts}.rs`
- **Problem**: Every block in the `for block in block_range` loop opens fresh read cursors (`cursor_read`) over `tables::Transactions` and `tables::Receipts`. For large sync ranges this creates hundreds of thousands of cursor objects and repeated walker initialisation, inflating CPU and allocations while blocking the Rayon workers.
- **Proposal**:
  - Hoist `cursor_read::<tables::Transactions>` and `cursor_read::<tables::Receipts>` before the block loop, reuse the cursor, and only reset the walker range per block.
  - When a block produces no rows, skip the walk without tearing the cursor down.
  - Guard with a small helper that rewinds the cursor to the start of the requested tx range to keep the change local.
- **Deliverables**:
  - Updated segment writer implementation with persistent cursors.
  - Benchmarks or profiling notes confirming reduced cursor creation counts (e.g. via `mdbx::table::Cursor` instrumentation or `perf` flamegraph).
- **Acceptance**:
  - Static-file production over a multi-million block range no longer triggers per-block cursor allocations.
  - No functional regressions in `cargo nextest run -p reth-static-file` and `make test-unit`.

## Issue 2: Batch `BlockBodyIndices` retrieval for producer segments

- **Area**: `crates/static-file/static-file/src/segments/{transactions,receipts}.rs`
- **Problem**: The producer fetches `block_body_indices(block)` individually for every block. Each call incurs a database seek even though the requested range is contiguous. This blocks the writer thread and prevents lookahead or overlap with static-file I/O.
- **Proposal**:
  - Use `BlockBodyIndicesProvider::block_body_indices_range` (see `crates/storage/storage-api/src/block_indices.rs`) to pull a chunk of indices in one shot.
  - Iterate over the returned slice in lockstep with the block loop.
  - Feed contiguous tx-number ranges to the segments, enabling prefetch of transaction/receipt walkers.
  - Optionally spawn a lightweight background task that preloads the next chunk while the current chunk is written (respecting MDBX read-only constraints).
- **Deliverables**:
  - Updated segment writers using range-based retrieval.
  - Benchmark demonstrating lower DB round-trip counts and improved end-to-end throughput.
- **Acceptance**:
  - For a range of N blocks we observe ≪ N calls into `block_body_indices`.
  - Static-file production throughput improves or remains steady with reduced read amplification.
  - Unit/integration suite passes.

## Issue 3: Avoid reloading jars during `StaticFileProvider::commit`

- **Area**: `crates/storage/provider/src/providers/static_file/manager.rs:600-655`
- **Problem**: `StaticFileProvider::commit` currently reloads the just-written jar from disk (`NippyJar::load`) to update indices. This duplicates work already performed by the writer (which has the in-memory header and offsets) and results in redundant mmap churn on every commit, especially when multiple segments finish together.
- **Proposal**:
  - Teach the writer to surface the updated `SegmentHeader` and offsets so `commit` can update indexes without reloading the jar.
  - Alternatively, defer the reload to a background maintenance task and update indices from the writer-provided metadata immediately.
  - Ensure synchronisation with existing notifier logic (`StaticFileProviderMetrics`, watchers).
- **Deliverables**:
  - Refactored commit path sharing state between writer and manager (document concurrency guarantees).
  - Benchmark covering repeated commits (e.g. `StaticFileProducer::run` over multiple segments) showing reduced I/O or mmap time.
- **Acceptance**:
  - `static_file_provider.commit()` no longer issues `NippyJar::load` in the hot path.
  - Observer metrics (if enabled) still reflect up-to-date segment maxima.
  - All existing tests (`make test-unit`, static-file specific suites) pass.

## Issue 4: Right-size range fetch buffers in `fetch_range_with_predicate`

- **Area**: `crates/storage/provider/src/providers/static_file/manager.rs:1100-1112`
- **Problem**: The helper reserves a fixed capacity of 100 items regardless of the requested range length. For large RPC reads (e.g., `transactions_by_tx_range`) this causes repeated reallocations and prevents Rayon-friendly chunking.
- **Proposal**:
  - Reserve min(range_len, sensible cap) to match the actual range size, or refactor to process explicit chunks and allocate per chunk.
  - Add benchmarks for large range reads to verify allocation behaviour (e.g., using `AllocatorStats`).
  - Consider exposing a streaming variant that returns an iterator without owning a `Vec`.
- **Deliverables**:
  - Adjusted capacity logic with tests covering small and large ranges.
  - Optional micro-benchmark demonstrating allocation reduction.
- **Acceptance**:
  - Profiling confirms the helper no longer reallocates heavily for large ranges.
  - All range-based provider APIs continue to behave identically.

## Issue 5: Pre-split range fetches per static file to remove retry loop

- **Area**: `crates/storage/provider/src/providers/static_file/manager.rs:1138-1160`
- **Problem**: When a range crosses static-file boundaries, the helper drops the current provider and retries mid-iteration. This adds branching overhead, forces lock contention on the dashmap, and prevents background prefetching of the next jar.
- **Proposal**:
  - Compute the set of static-file slices covering the requested range up-front (using `find_fixed_range` and the index metadata).
  - Iterate slice-by-slice, allowing the caller to prefetch the next jar while processing the current one.
  - Expose a small iterator struct to preserve existing API signatures while avoiding retry logic.
- **Deliverables**:
  - Refactored range fetch that walks deterministic slices.
  - Additional tests spanning multiple jar boundaries to ensure continuity.
  - Optional performance comparison highlighting reduced lock churn.
- **Acceptance**:
  - Range fetches no longer invoke the retry branch for cross-file reads.
  - Downstream calls (e.g., `transactions_by_tx_range`, `headers_range`) work unchanged.
  - Performance profiling shows fewer provider lookups/lock acquisitions.

# Filing Guidance

For each issue:

1. Copy the section into a GitHub issue.
2. Add the `area/storage` and `perf` labels.
3. Reference this document for shared context.
4. Link any profiling artefacts (flamegraphs, `perf stat`, etc.) collected during implementation.
