Iteration 5 is rejected. The LIFO-reclaim candidate increases persistence wait 9.25% while payload metrics remain neutral. The added save-blocks time is concentrated in `write_state` and hashed-state persistence; commit and commit-GC costs are nearly unchanged.

This unprofiled six-pair run compares original `eadb887f1c1698e7bd932f60a0c654dccccd081f` with retained trie changes plus LIFO reclaim at `4355f17739405d5eb13088b45bbdcae4ba15c52d`. It replays 500 measured blocks after 125 warmup blocks. Both diagnostic summaries reproduce the original official changes exactly; no acceptance configuration or thresholds changed.

| Official metric | Baseline | Candidate | Change, 95% CI half-width | Verdict |
|---|---:|---:|---:|---|
| Mean newPayload | 24.126 ms | 23.943 ms | −0.758% ±0.727% | Neutral |
| P90 | 37.856 ms | 37.874 ms | +0.049% ±1.296% | Neutral |
| P99 | 70.025 ms | 73.850 ms | +5.462% ±5.823% | Neutral |
| Reported wall clock | 72.788 s | 72.346 s | −0.606% ±0.707% | Neutral |
| Persistence wait | 27.156 ms | 29.668 ms | +9.250% ±1.331% | Regression |
| MDBX save-blocks work | 28.840 ms/block | 31.366 ms/block | +8.757% ±0.872% | Regression |
| Save-blocks MDBX commit | 15.378 ms/block | 15.414 ms/block | +0.230% ±1.613% | Neutral |

The p99 increase is retained despite its neutral official classification. Reported wall clock sums server newPayload plus client forkchoiceUpdated times across all measured rows; it excludes the separately reported persistence wait and is not elapsed replay time. Supplementary actual replay duration rises from 27.099 s to 28.346 s (+4.600% ±0.586%).

| Diagnostic phase | Baseline per canonical block | Candidate | Change, 95% CI half-width |
|---|---:|---:|---:|
| Final `root_with_updates` | 343.701 µs | 318.783 µs | −7.250% ±1.738% |
| Incremental sparse-trie updates | 8.932 ms | 8.791 ms | −1.579% ±0.694% |
| Multiproof reveal | 4.538 ms | 4.510 ms | −0.607% ±1.722% |
| Total sparse-trie task | 23.308 ms | 23.151 ms | −0.677% ±0.843% |
| Trie-persistence phase | 19.767 ms | 19.575 ms | −0.972% ±0.886% |
| Hashed-state persistence | 8.406 ms | 9.565 ms | +13.787% ±1.301% |
| `write_state` | 0.601 ms | 2.153 ms | +258.096% ±17.997% |
| Insert-block phase | 29.772 µs | 36.938 µs | +24.068% ±8.494% |
| Save-blocks total | 28.842 ms | 31.368 ms | +8.759% ±0.892% |

`write_state` includes state and enabled changeset/receipt writes. Hashed-state timing includes collecting and merging the batch as well as writes. Trie persistence includes deferred-data waiting, merging and node writes. The added 1.552 ms in `write_state`, 1.159 ms in hashed-state work, and smaller component changes, offset by 0.192 ms saved in trie work, explain 2.5254 ms of the 2.5263 ms total increase. Commit decomposition shows only +0.0346 ms whole-commit time and +0.0036 ms GC wall time per canonical block. Those all-read-write-commit metrics also include pruning, and GC CPU time overlaps GC wall time. Neither component supports attributing the large regression to commit GC.

Completed-work counters report 3,008 baseline blocks in 376 batches versus 3,022 candidate blocks in 378 batches. Normalized trie time is 19.655 versus 19.380 ms per completed block (−1.397% ±0.587%). Every run has complete phase metrics, monotonic counters, and equal trie-call/batch counts. The workload's 500 measured block hashes match the payload in all 12 processes, and the payload file is byte-identical to iteration 2. Ordered block/gas/transaction counts also match.

The independent append-log audit strictly agrees in 11 of 12 runs. Baseline-6 has a seven-block append logged at 12:00:21.309005 UTC, five microseconds after the first scrape's integer-millisecond timestamp. That scrape already counts the append. The strict log window therefore contains 503 blocks/63 batches, versus sampled deltas of 496/62. Excluding the same-millisecond append reconciles the counts, consistent with timestamp granularity; no adjustment is applied, and the mismatch remains explicitly flagged in `log-boundary-ambiguity.json`. Other persistence windows also cover differing boundary blocks. Normalization does not establish identical work or a durable final drain.

| Memory: mean per-run sampled peak | Baseline | Candidate | Difference |
|---|---:|---:|---:|
| Process RSS | 10,491.2 MiB | 9,815.6 MiB | −675.6 MiB (−6.44%) |
| Jemalloc allocated | 3,977.3 MiB | 3,971.7 MiB | −5.6 MiB (−0.14%) |
| Jemalloc active | 4,076.8 MiB | 4,071.0 MiB | −5.8 MiB (−0.14%) |
| Jemalloc resident | 4,333.6 MiB | 4,313.4 MiB | −20.2 MiB (−0.47%) |

The RSS difference is already −672.2 MiB at the first sample and −680.8 MiB at the last available sample; it is not solely a change during measured replay. Baseline has 136–137 samples per run, candidate 141–144, at roughly 200 ms intervals. Samples cover replay but stop 8.9–9.4 seconds before the target-metrics window ends, with no later raw gauge samples available. These are sampled peaks and last sampled values, not true high-water marks or post-drain memory. RSS includes mapped pages and overlaps allocator accounting; it cannot be added to jemalloc values or treated as total host/cgroup memory. Reduced resident footprint did not produce faster persistence.

The newPayload-thread major-fault metric changes −0.41% ±1.15%, and its minor-fault metric +7.91% ±7.81%; both are neutral. They do not measure persistence-thread page faults and cannot establish the cause of the write-state regression.

A bounded full-archive inventory of baseline-1 (1,279,825 samples) and feature-1 (1,345,061 samples) found 912 metric names each and no MDBX mincore, prefault, pgops or page-operation metrics. Source inspection confirms these counters are not exported by `DatabaseEnv::gauge_metrics`. The other ten archives were not independently inventoried for this check. The older CPU profile motivates a locality hypothesis, but this run supplies no probe-count, per-probe-cost, or persistence-thread fault measurement to prove it.

Original artifacts are unchanged. `iteration-5.json`, the phase and extra-save-phase directories, commit decomposition, completed-work/log audits, and memory audit retain the quantitative evidence. The rejected candidate remains diagnostic; iteration 6 is a separate pending experiment with unchanged acceptance rules.
