# Compact arena capacity 2: held; not adopted

Candidate `8f5a8fb98cd01b4614b730a34944f3253e79d80e` reduces the branch-child SmallVec inline capacity from four to two and makes its index implementation generic over capacity. It is based on retained production source `2042e313a2b7cb0e7606dceeca75005612702d8f`. The frozen f4fb30dfaf control has the same sparse-trie source as that parent. DWARF type inspection of the actual binaries reports the arena node and branch as 248→176 bytes (−29.03%); a child remains 36 bytes. The actual `slotmap::basic::Slot<ArenaSparseNode>` entry stride is 256→184 bytes (−28.13%), including slot metadata/alignment. These are type/entry sizes, not total allocation, RSS or an end-to-end memory improvement.

**Hold; not adopted.** The standard root matrix has lower candidate medians in 46 of 56 comparisons, including useful large retained-update gains. Fresh-proof revelation exposes a repeatable regression: revealing all 1,000 leaves is 11.98% slower with one worker and 12.49% slower with 15 workers, with disjoint process-median interquartile ranges in both cases. Both arena entry stride/alignment and small-vector spills beyond two children change. These are plausible tradeoffs, but this benchmark does not independently isolate allocation/cache effects or establish a production effect. Capacity 3 is being tested as a separate candidate; it does not replace or erase these results.

Both matrices use rustc 1.98.1, profiling, `-C target-cpu=native`, assembly Keccak and global Keccak cache, with the system allocator. Two ABBA rounds at 100 samples/case give four process medians per variant and worker count. Actual metadata allows CPUs 1–15 for both one-worker and 15-worker processes; the one-worker processes are not pinned to CPU 1 alone. Reports give medians and inclusive quartiles of process medians, not confidence intervals. The full 56 root and 12 reveal comparisons, including unfavorable controls, remain in the CSVs.

| Workers | Case | Baseline µs [p25,p75] | Capacity 2 µs [p25,p75] | Time change |
| ---: | --- | ---: | ---: | ---: |
| 1 | update_root/32768/32768/retain | 16767.807 [16674.207, 16876.887] | 15979.453 [15916.789, 16145.630] | -4.70% |
| 15 | update_root/32768/32768/retain | 5642.768 [5545.883, 5754.047] | 5393.615 [5353.821, 5480.514] | -4.42% |
| 1 | update_root/32768/3276/retain | 3829.572 [3824.059, 3835.608] | 3696.613 [3694.651, 3699.003] | -3.47% |
| 15 | update_root/32768/3276/retain | 998.337 [985.784, 1013.337] | 963.513 [960.993, 965.306] | -3.49% |
| 1 | update_root/1000/1000/discard | 351.118 [350.527, 352.343] | 356.984 [350.187, 421.693] | +1.67% |
| 15 | update_root/1000/1000/discard | 257.228 [253.443, 265.052] | 268.554 [258.588, 271.354] | +4.40% |
| 1 | reveal/1000/1000 | 278.017 [276.517, 279.212] | 311.330 [310.017, 312.663] | +11.98% |
| 1 | reveal/32768/32768 | 12047.987 [12009.377, 12070.259] | 9931.331 [9922.630, 9937.811] | -17.57% |
| 15 | reveal/1000/1000 | 278.087 [276.700, 280.069] | 312.808 [310.946, 316.485] | +12.49% |
| 15 | reveal/32768/32768 | 13153.953 [12410.954, 13898.050] | 12159.775 [12141.691, 12181.811] | -7.56% |

The one-worker `update_root/1000/1000/discard` candidate has process medians 351.008, 597.891, 347.722, 362.960 µs. The 597.891 µs observation remains in the original data and summary; its own sample quartiles are 346.681–600.286 µs. It is not discarded, relabeled as a different workload, or used alone to establish a regression. The 15-worker version has a 4.40% median slowdown with overlapping interquartile ranges. Other small unfavorable cases and nanosecond-scale ties remain visible in `root-summary.csv`.

The standard kernel uses synthetic 1,000/32,768-leaf storage tries, cached or dirty roots and changed-value update+root work with retained/discarded updates. Fixture construction and cloning are outside timing. The separate 57-line reveal harness builds proofs with TrieTestHarness; each sample clones a root-only trie and proof buffer outside timing, then times only `reveal_nodes`. It verifies the complete root and selected leaf values afterward. Five warmups are excluded. It covers 1 leaf, 10% or all leaves, and does not model retained-cache routing or repeated overlapping proofs. Both matrices repeatedly use warm fixtures and shared Keccak-cache keys; they contain no database I/O or execution-client workload.

All 180 sparse tests passed. The deterministic seed 1 differential sweep passed 10,000 sequences, 4,448,976 operations, 641,072 independent root checks, 255,536 persisted-node checks, 100,022 proof rounds, 90,000 reconstructed reopens, 40,000 prunes and 65,536 occupancy masks. These reconstructed reopens are not actual MDBX recovery tests. Validation and source hashes are preserved; full logs and frozen binaries remain in the local benchmark workspace. Reveal binary metadata records the pre-commit parent HEAD because the candidate patch was built before it was committed; `production.patch` and the recorded final commit identify that change.

No production branch, normal-workload verdict, benchmark threshold or main implementation prose was changed for this archive.
