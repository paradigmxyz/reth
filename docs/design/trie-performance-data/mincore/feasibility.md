# MDBX_USE_MINCORE=0: read-only feasibility

This note records the initial assessment before the separately authorized diagnostic builds and measurements.

A bounded diagnostic is justified, with a substantial predicted warm-page cost. No implementation, build, or measurement was performed. The current sort/metrics candidate remains the primary experiment.

## Supported configuration

The bundled MDBX explicitly validates `MDBX_USE_MINCORE` as 0 or 1 (`mdbx.c:1697`), and upstream's bundled CMake configuration exposes it as an advanced option (`CMakeLists.txt:493`). Reth's build uses `cc::Build` directly, not CMake. `crates/storage/libmdbx-rs/mdbx-sys/build.rs:15–48` does not set this macro, so a diagnostic can pass `CFLAGS=-DMDBX_USE_MINCORE=0` without modifying any source. Locked cc 1.2.67 documents CFLAGS and tracks environment changes. For strictly matched builds, use explicit `=1` and `=0` controls, identical other flags, and separate target directories. The final compiler command must be checked for target-specific CFLAGS overrides.

If later justified, the production configuration could be a single `cc.define("MDBX_USE_MINCORE", "0")` in the non-vendored build.rs. No vendored edit or Rust FFI/API change is necessary. `mdbx_build.options` embeds `MDBX_USE_MINCORE=...` (`mdbx.c:24339`), providing executable provenance. This macro does not change the persistent format or the shared lock structure; the mincore cache fields remain present.

## Exact behavior

With the default enabled, `mincore_probe` checks a four-window cache, each window covering 64 units; a unit is the larger of MDBX and OS page size. A miss queries mincore and records residency. `bit_tas` also marks a probed page present for subsequent probes (`mdbx.c:21654–21743`).

With the macro disabled, `mincore_probe` always returns false. In `page_alloc_finalize` (`mdbx.c:22372–22465`), an active WRITEMAP prefault path therefore always attempts pwrite for a single allocated page or pwritev batches for multiple pages. This includes already resident destination pages. It does not write on every logical cursor mutation: it acts when pages are allocated/finalized, and the loose-page reuse fast path can bypass it. The existing fallback/error behavior is unchanged. This is a change to an I/O optimization, not a durability mode change.

The cold-destination overwrite path remains active: the page is populated through the file handle before the mapping is touched, avoiding the existing unnecessary read of obsolete destination contents on supported coherent file-cache systems. Source-page reads needed to copy/modify the tree remain. All existing activation conditions still apply: WRITEMAP, the runtime prefault option, coherent mapped/file writes, non-incore environment, and GC/readahead heuristics. Shadow-page mode never enters this prefault block, so the compile option has no relevant effect there.

## Readahead interaction

`gc_alloc_ex` resets the transaction activation flag from the runtime option, then disables it when the GC has no branch pages and `geo.now < 1234`, or when readahead is enabled and `pgno + num < readahead_edge` (`mdbx.c:22543–22557`). The compile option does not override either shortcut. At this point in the normal GC search path, pgno is still zero; the source should not be described as checking the eventual selected destination page. Allocations from an already populated reclaimed list reuse the transaction flag.

Reth explicitly sets `no_rdahead: true` (`crates/storage/db/src/implementation/mdbx/mod.rs:467–476`), so readahead does not normally suppress the cost for its writer. A standalone default-MDBX or tiny fresh-database benchmark could exercise the shortcut and misleadingly report no effect. The existing large reth-db fixture is preferable, with baseline mincore and prefault counters checked.

## Measured evidence and expected tradeoff

The existing candidate-33 profile assigns 1.512 s inclusive, 32.84% of measured trie-persistence CPU, to __mincore_unmapped_range over 250 blocks. This overlaps its nested page-cache lookup functions and is not a wall-clock savings estimate. Removing the 64-page probes is a directly supported hypothesis, but replacement pwrite calls also enter the kernel/page cache.

Prior native prefault diagnostics (`bench-work/trie-derek-20260904/prefault-experiment/report.md`) used 141 mincore calls per large replacement transaction. Warm transactions made zero prefault writes; cold transactions made about 8,919 prefault operations. Disabling mincore is expected to add writes for roughly that many warm allocated pages (about 35 MiB at 4 KiB/page), instead of merely removing 141 probes. Exact write count depends on single/multi-page allocation and must be measured. The bundled comments explicitly warn that redundant prefault writes can be expensive (`mdbx.c:22404–22412`). Cold performance may improve by avoiding probe lookups while retaining writes already required by the baseline; warm performance can regress considerably.

## Smallest useful experiment

Reuse the existing packed account/storage writer harness, with identical native Rust 1.98.1 flags, FIFO reclamation, WRITEMAP, readahead disabled, and the current runtime prefault default. Compare only explicit compile settings 1 and 0. Measure (1) warm file cache with reopened mappings, (2) warm persistent mappings, and (3) verified cold disposable files. Keep preparation/eviction outside timing, preserve exact snapshots plus durable commit/abort/reopen checks, and record mincore, prefault, minor/major faults, write/commit times, and build options. Candidate MDBX mincore count should be zero while cold prefault writes remain; the harness's separate mincore residency verification should remain enabled.

A short paired warm/cold screen is enough to reject a material warm regression before any larger benchmark. If it passes, use repeated ABBA measurements before considering Derek. Do not combine it with LIFO or shadow pages: those alter the mechanism and would obscure the attribution. No claim of benefit is justified from code/profile evidence alone.
