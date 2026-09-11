# Residency-cache experiment

This draft enables `MDBX_MINCORE_DIAGNOSTICS=1` in `build.rs`, on top of the
four-entry cache insertion fix. It leaves cache capacity, query shape, returned
residency decisions, and the lock-file layout unchanged. This is diagnostic code,
not a proposal to enable transaction logging in production.

Counters are process-local, serialized with the existing cache operations, and
intended for one writer environment per database. Other processes can mutate the
shared cache without updating this environment's diagnostic metadata; do not use
the utilization/ghost results for multi-writer-environment experiments.

There are no per-probe allocations, atomics, timers, or log messages. A hit updates
counters and a four-entry shadow usage mask. On a miss, diagnostics search at most
16 recently evicted ranges and count residency bits in the returned vector. That
additional work is measured against an uninstrumented baseline in the benchmark.

Each completed or aborted top-level write transaction emits one cumulative JSON
snapshot prefixed `MDBX_MINCORE_DIAG` to stderr. It includes commit-time GC probes.
The final snapshot covers startup, warmup, measured blocks, and any subsequent
write transactions. Do not sum snapshots or describe them as measured-window-only.
Read-only transactions emit nothing. No block numbers or keys are logged.

Fields:

- `probes`, `hits[0..3]`, `misses`, `errors`: hit position before promotion; misses
  are syscall attempts, including errors. `probes = sum(hits) + misses`.
- `resident_hits`, `nonresident_hits`: cached decision before the existing
  optimistic promotion of that bit to resident. `resident_misses` and
  `nonresident_misses` describe the requested unit from a successful syscall.
  These are hints, not independently verified cache accuracy.
- `repeated_hits`: probes of a bit already visited in that cached window.
  `untracked_hits` should be zero in the single-environment experiment.
- `queried_os_pages`, `resident_os_pages`: successful queries only, counting OS
  pages even when one DB page spans several OS pages. `queried_units` counts
  max(DB page size, OS page size) units. Repeated queries count again.
- `all_resident`, `none_resident`, `mixed`: classify returned window unit masks.
  A unit is resident only if all of its OS pages are resident. `clipped_queries`
  counts queries shorter than 64 units at the mapping tail.
- `evictions`, `invalidated_windows`, `clears`: replacement versus explicit cache
  clearing, including clears with no live window.
- `completed_windows`, `completed_units`, `used_units`, `usage_hist[0..64]`:
  observed query-window lifetimes. Snapshots also account for live windows without
  evicting or otherwise changing them. `usage_hist[n]` counts windows where exactly
  n distinct bits have been requested. This is usage before eviction/clear or as
  of the snapshot, not a global count of unique DB pages. Live histogram buckets
  can move between snapshots, so histogram deltas can be negative.
- `offset_hits[0..63]`: cached probes by relative position in the query window;
  excludes the initial miss at offset zero, includes repeated hits.
- `ghost_hits[0..15]`: misses falling in a recently evicted query range, indexed
  by eviction age (zero is newest). `ghost_misses` means no match. Ghosts clear on
  invalidation. This is a capacity-reuse indicator, **not** a simulated larger
  cache's hit rate: real query windows, overlap, and replacement would change.

Run standalone Linux tests from this directory:

```sh
cc -O1 -pthread tests/mincore_cache.c -o /tmp/mincore_cache_test
/tmp/mincore_cache_test
cc -O1 -pthread tests/mincore_diagnostics.c -o /tmp/mincore_diagnostics_test
/tmp/mincore_diagnostics_test
```

Summarize the last snapshot in each benchmark node log:

```sh
uv run summarize_mincore.py /path/to/bench-results/feature-*/node.log
```

Use observed hit positions and ghost reuse to select capacity experiments, and
window utilization plus hit offsets to select query-size experiments. Each
candidate still requires an uninstrumented paired `save_blocks` benchmark; these
metrics cannot predict the syscall-cost/write-avoidance tradeoff by themselves.
