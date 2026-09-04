# Compact arena capacity 3: held pending runtime evidence

Candidate `b65d76b06d930fa39fe70d842812490bba350222` has inline capacity three for branch children, plus capacity-generic indexing. Relative to retained production source `2042e313a2b7cb0e7606dceeca75005612702d8f`, those are the only two source changes. It follows capacity-2 commit 8f5a8fb98c, but the control is the original capacity-four implementation. The frozen f4fb30dfaf control has the same sparse-trie source as 2042. DWARF inspection reports arena node types of 248→216 bytes (−12.90%), versus 176 for the separate capacity-2 experiment. Actual `slotmap::basic::Slot<ArenaSparseNode>` entry strides are 256→224 bytes (−12.50%), versus 184 for capacity 2. Extra spilled children allocate separately. Both stride/alignment and child spills change; these sizes neither isolate allocation/cache effects nor measure total heap or RSS.

**Hold; not adopted.** No Derek result or representative runtime qualification exists for this capacity change, and it is not included on main. Forty of 56 root comparisons and seven of 12 reveal comparisons have lower candidate medians. Large retained update+root work improves 2.68% with one worker and 1.92% with 15 workers. Fresh all-leaf revelation into a 1,000-leaf trie is 1.60%/2.94% slower; its quartile ranges overlap for one worker but are disjoint for 15 workers. This is a mixed component result, not a clear overall improvement. The separately recorded capacity-2 results remain unchanged; these runs are not a randomized direct comparison of capacity 2 versus 3.

The native rustc 1.98.1 profiling builds use assembly Keccak, global Keccak cache and the system allocator. Every run allows CPUs 1–15, including the one-worker runs. Each matrix uses two ABBA rounds with 100 samples/case after five warmups, yielding four process medians per variant and worker count. The report shows medians and inclusive quartiles of process medians, not confidence intervals. All 56 root and 12 reveal comparisons and their process rows are preserved.

| Workers | Case | Baseline µs [p25,p75] | Capacity 3 µs [p25,p75] | Time change |
| ---: | --- | ---: | ---: | ---: |
| 1 | root_dirty/32768/32768/retain | 10219.665 [9993.693, 10466.398] | 9681.886 [9664.275, 9690.824] | -5.26% |
| 1 | update_root/1000/1000/retain | 373.911 [373.398, 375.728] | 377.283 [376.083, 377.596] | +0.90% |
| 1 | update_root/32768/32768/retain | 16747.751 [16730.903, 16822.805] | 16298.428 [16245.648, 16408.953] | -2.68% |
| 15 | root_dirty/32768/32768/retain | 1196.005 [1166.780, 1248.999] | 1173.131 [1129.211, 1238.505] | -1.91% |
| 15 | update_root/1000/1000/retain | 291.627 [290.152, 293.075] | 291.096 [290.513, 293.235] | -0.18% |
| 15 | update_root/32768/32768/retain | 5732.873 [5649.812, 5854.783] | 5622.927 [5493.930, 5705.101] | -1.92% |
| 1 | reveal/1000/1000 | 280.411 [277.198, 284.356] | 284.909 [283.798, 286.072] | +1.60% |
| 1 | reveal/32768/32768 | 11838.441 [11804.622, 11856.325] | 11510.952 [11481.830, 11533.126] | -2.77% |
| 15 | reveal/1000/1000 | 277.966 [276.255, 280.401] | 286.147 [284.794, 288.381] | +2.94% |
| 15 | reveal/32768/32768 | 13052.314 [12414.923, 13736.450] | 11842.549 [11399.870, 11980.806] | -9.27% |

Cached-root and single-proof controls remain visible in absolute nanoseconds. Large percentages at this scale do not by themselves establish an application-level regression or benefit.

| Workers | Case | Baseline ns | Capacity 3 ns | Difference ns | Time change |
| ---: | --- | ---: | ---: | ---: | ---: |
| 1 | root_cached/1000/discard | 40.0 | 40.0 | +0.0 | -0.00% |
| 1 | root_cached/1000/retain | 40.0 | 40.0 | +0.0 | -0.00% |
| 1 | root_cached/32768/discard | 80.5 | 105.5 | +25.0 | +31.06% |
| 1 | root_cached/32768/retain | 70.0 | 85.0 | +15.0 | +21.43% |
| 15 | root_cached/1000/discard | 50.5 | 40.0 | -10.5 | -20.79% |
| 15 | root_cached/1000/retain | 40.0 | 40.0 | +0.0 | -0.00% |
| 15 | root_cached/32768/discard | 75.0 | 65.0 | -10.0 | -13.33% |
| 15 | root_cached/32768/retain | 80.0 | 75.0 | -5.0 | -6.25% |
| 1 | reveal/1000/1 | 455.5 | 471.0 | +15.5 | +3.40% |
| 15 | reveal/1000/1 | 425.5 | 460.5 | +35.0 | +8.23% |

Other unfavorable controls include the 15-worker 1,000-leaf dirty-root case with 100 retained updates (+2.38%) and all 1,000 dirty/discard updates (+2.75%). One-worker combined all 1,000 retained updates is 0.90% slower; 15-worker single retained updates is 0.67% slower. Nothing is removed or reweighted in the archived summaries.

These are synthetic sparse storage tries with repeatedly warmed fixtures and Keccak-cache keys. Root cases cover 1,000/32,768 leaves, cached/dirty roots, and changed-value update+root work with retained/discarded updates. The reveal harness times insertion of fresh proofs into a root-only trie and checks the full root and selected leaf values afterward; it does not model retained overlapping proofs or outer storage-cache routing. Fixture/proof construction, cloning and correctness checks are excluded from timing. There is no database I/O, execution workload, or end-to-end memory measurement.

All 180 sparse tests, the six reveal smoke cases and formatting passed. The deterministic seed 1 differential sweep passed 10,000 sequences: 4,448,976 operations, 641,072 root checks, 255,536 persisted-node checks, 100,022 proof rounds, 90,000 in-memory rebuilds, 40,000 prunes and 65,536 occupancy masks. These rebuilds are not actual durable MDBX reopens. Exact source, binary, matrix and validation hashes are retained; full logs and binaries stay in the local benchmark workspace. The smoke timings are correctness evidence only and are not mixed into the measured matrix.

The ongoing predeclared large-block Derek run tests retained 2042/f294 source, without either arena-capacity experiment. Its future result must not be attributed to this candidate. Main implementation prose and all canonical-workload verdicts remain unchanged.
