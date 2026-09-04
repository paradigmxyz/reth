Original versus retained trie code has lower medians in 54 of 56 native/cache-enabled kernel cases. All 24 dirty-root comparisons improve. Two update-plus-root cases are slightly slower (0.08% and 0.65%), with overlapping process quartiles; those observations are retained rather than counted as wins. There are no confidence intervals or significance claims for this microbenchmark.

Both sides use Rust 1.98.1, profiling, `target-cpu=native`, ASM Keccak and process-global Keccak caching. There are two ABBA rounds at Rayon 1 (CPU 1) and Rayon 15 (CPUs 1–15), four processes per variant/thread count, 100 timed samples per case. Every root is checked with independent Alloy HashBuilder, and all 16 process outputs have identical case/root sets. Build/source/log/executable hashes are recorded in `build-metadata.json`; raw process summaries and complete quartiles are preserved.

| Case group | Rayon 1 median reduction | Rayon 15 median reduction |
|---|---:|---:|
| Cached root | 88.44% to 90.02% | 84.10% to 92.71% |
| Dirty root only | 1.46% to 16.82% | 0.71% to 30.19% |
| Apply updates + root + extract updates | -0.08% to 10.81% | -0.65% to 16.32% |

Selected 32,768-leaf cases retaining persistence updates:

| Work | Workers | Baseline | Retained | Reduction |
|---|---:|---:|---:|---:|
| root_dirty/32768/3276/retain | 1 | 3.1809 ms | 3.0039 ms | +5.56% |
| root_dirty/32768/32768/retain | 1 | 9.5369 ms | 9.1300 ms | +4.27% |
| update_root/32768/3276/retain | 1 | 4.1080 ms | 3.9904 ms | +2.86% |
| update_root/32768/32768/retain | 1 | 18.1727 ms | 17.9161 ms | +1.41% |
| root_dirty/32768/3276/retain | 15 | 0.5957 ms | 0.5237 ms | +12.08% |
| root_dirty/32768/32768/retain | 15 | 1.2346 ms | 1.1154 ms | +9.65% |
| update_root/32768/3276/retain | 15 | 1.3509 ms | 1.3084 ms | +3.15% |
| update_root/32768/32768/retain | 15 | 9.4365 ms | 9.4978 ms | -0.65% |

Slower cases and observed process quartiles:

| Workers / case | Baseline median [Q1,Q3] | Retained median [Q1,Q3] |
|---|---:|---:|
| 1 / update_root/32768/1/discard | 6.628 [6.529, 6.721] µs | 6.633 [6.598, 6.665] µs |
| 15 / update_root/32768/32768/retain | 9436.538 [9394.491, 9511.009] µs | 9497.782 [9461.960, 9541.427] µs |

These results use the same hash features as the node, but repeated input values and fixture/reference hashing warm the process-global cache before measurement. Cache hit rates are unavailable. Setup, input allocations, clones and root checks are excluded. The harness uses the system allocator and performs no database I/O. Very small cached-root timings include timer overhead and should not drive an end-to-end claim. See `README.md` for cache details.

The earlier Rust 1.96.1 ASM-only / Rayon 1,4 results used a different hash-cache configuration and cannot be compared directly with these absolute timings. This native/cache matrix provides a matching-compiler kernel check. The retained candidate’s unprofiled Derek runs remained neutral; these kernel gains do not change that official verdict.
