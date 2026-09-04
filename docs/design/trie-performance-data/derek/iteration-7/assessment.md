# Derek 7: neutral; pending-account root filter not adopted

Run [33874668983](https://github.com/paradigmxyz/reth/actions/runs/33874668983) compares baseline `eadb887f1c1698e7bd932f60a0c654dccccd081f` with `8c2ab90521c046f00a1f20b56adabe623c1e3fce`. The candidate adds a pending-account storage-root eligibility filter to retained production source `2042e313a2b7cb0e7606dceeca75005612702d8f`: drained roots are computed only for accounts that still have a pending account update. Original FIFO reclamation, WRITEMAP, prefault writes, and synchronization settings remain in use.

**Not adopted.** The six-pair run establishes no clear overall or persistence improvement under the unchanged Derek rules. Main remains at the retained 2042 production source and f294 documentation checkpoint. Full candidate validation passed (184 engine tests, 3,821 workspace tests with 38 skips, strict Clippy and docs); one RPC-port conflict passed on retry. Correctness validation does not establish performance benefit. The remote comparison is against original eadb, so it also does not isolate the filter's incremental effect against its 2042 parent.

Each variant replays 500 measured mainnet blocks after 125 warmups in six independent processes, with metrics enabled and profiling disabled. Original run-cluster bootstrap intervals and practical floors remain authoritative.

| Official headline | Candidate change | Reported 95% half-width | Practical floor | Verdict |
| --- | ---: | ---: | ---: | --- |
| mean | +0.1836% | 0.1820% | 1.20% | neutral |
| p50 | +0.0671% | 0.8218% | 1.20% | neutral |
| p90 | +0.8772% | 1.2712% | 1.35% | neutral |
| p99 | +2.7293% | 1.6024% | 5.00% | neutral |
| mgas_s | -0.0090% | 0.2272% | 1.20% | neutral |
| wall_clock | +0.1237% | 0.3407% | 0.70% | neutral |
| persist_wait | -2.6911% | 1.1764% | 5.00% | neutral |

Mean newPayload latency is 23.7115→23.7550 ms. Reported wall-clock is 71.6485→71.7371 seconds across 3,000 rows per variant: a sum of newPayload and forkchoice latencies, not elapsed replay time. Persistence wait is 26.453→25.741 ms (−2.69%), below the original 5% floor. With `wait-for-persistence=never`, it measures engine backpressure rather than a final durable drain. The p99 increase of 2.73% remains in the record despite its neutral classification.

Supplementary actual replay duration from `report.run_stats.duration_ms` averages 26.5765→26.2863 seconds (−1.092% ±0.413%, using the same bootstrap primitive). This diagnostic does not replace the official neutral verdict or its thresholds.

## Root and persistence phases

Cumulative histogram seconds are divided by the sampled canonical-height delta and averaged equally across processes. All diagnostic duration targets retain a 5% floor. Negative changes mean less elapsed time.

| Phase | Baseline ms/block | Candidate ms/block | Change | Reported 95% half-width | Diagnostic verdict |
| --- | ---: | ---: | ---: | ---: | --- |
| final_update_duration_histogram | 0.332981 | 0.311098 | -6.5717% | 2.9599% | neutral |
| process_updates_duration_histogram | 8.658034 | 8.263053 | -4.5620% | 0.7573% | neutral |
| total_duration_histogram | 22.926550 | 22.992372 | +0.2871% | 0.2530% | neutral |
| reveal_multiproof_duration_histogram | 4.518467 | 4.520772 | +0.0510% | 1.2049% | neutral |
| channel_wait_duration_histogram | 4.897316 | 5.242967 | +7.0580% | 1.0760% | bad |
| write_trie_updates | 19.409065 | 19.050115 | -1.8494% | 0.7508% | neutral |
| total | 28.355381 | 27.944875 | -1.4477% | 0.6157% | neutral |
| commit_mdbx | 14.956123 | 14.842905 | -0.7570% | 1.3928% | neutral |
| write_hashed_state | 8.270732 | 8.214057 | -0.6852% | 0.9921% | neutral |
| write_state | 0.603589 | 0.612309 | +1.4446% | 8.5752% | neutral |
| insert_block | 0.033112 | 0.030745 | -7.1504% | 8.5057% | neutral |
| update_history_indices | 0.000343 | 0.000354 | +3.1893% | 3.5862% | neutral |
| update_pipeline_stages | 0.003197 | 0.003125 | -2.2399% | 2.8694% | neutral |

Process-update time falls 0.395 ms/block, while channel waiting rises 0.346 ms/block. Final-root duration falls 0.022 ms/block, but its interval does not clear the 5% floor. Total root-task duration is flat. These are overlapping elapsed phases, not measurements of CPU saved by the filter. The official `hash-post-state` worker target is 319.150→268.578 µs/block (−15.846% ±5.924%, a qualifying target improvement); in sparse mode its closure normally waits to receive already hashed state. Its elapsed duration is not direct hashing or sorting CPU. See `worker-source-audit.md`.

Process-update calls increase 84.04→89.98 per canonical block (+7.07%) and reveal calls 36.03→37.85 (+5.04%). Account/storage cache misses increase 8.90%/7.51%; hits increase 1.16%/0.65%. These counters cover offered updates across passes and retries, not unique keys or individual sort sizes. No scheduled-root-count histogram establishes how many prewarm-only roots were avoided. The source trace-span `n` is not available in this unprofiled replay.

All eight root-work histograms are present and monotonic in twelve processes. Baseline-6 has a one-observation endpoint skew: account hits and storage misses include 500 observations, while account misses, storage hits, final-root and total-root include 499. The other eleven processes have matching task-observation counts. Separate records are not an atomic snapshot. The raw counts remain unchanged and this boundary uncertainty limits exact task-to-task comparisons.

The direct trie-persistence phase includes deferred-data waiting, merging updates and node writes. Hashed-state time likewise includes collection and merging; save-total excludes the later commit. These phases must not be added to overlapping root/worker timings or described as isolated node-write CPU.

## Completed work and commit audit

Both variants complete **3,024 persisted blocks in 378 batches**, with 504 blocks and 63 eight-block batches in each process. All twelve strict append-log/counter checks agree, and aggregate logged block coverage is identical. There are no missing work metrics, negative deltas or trie-call/batch-count mismatches. The canonical scrape deltas are 497–499 because metric endpoints differ from the 500 payload endpoints.

Per completed persisted block, trie-persistence time is 19.1908→18.8423 ms (−1.816% ±0.789%). Equal completed work strengthens this small phase comparison; it still does not clear the diagnostic 5% floor or establish durable throughput. Logged append completion precedes transaction commit, and replay does not drain persistence.

Across all read-write commits, including other writers such as pruning, whole commit time is 14.9333→14.8258 ms/canonical block. Synchronization is 14.8495→14.7426 ms, GC wall time 0.07982→0.07919 ms, and GC CPU 0.01427→0.01447 ms. All eight commit-component counts match within each process (124–126 commits). GC CPU overlaps GC wall time; explicit-write time reported as zero under WRITEMAP does not mean there was no I/O. These data do not attribute the small timing differences to page faults or residency probes.

## Workload, runtime provenance and memory

All twelve reports and received-block logs match the same ordered 500-block payload, heights 25,883,613–25,884,112, 150,862 transactions and 15,020,694,260 gas. Its SHA-256 is `70c2694e3941f5de87c466907b13f2f3bbb93d4f5bc1bcbb6455485600573c50`, also matching iteration 2. Report source labels match the official commits. This checks reported/logged hashes and work, not an independent recomputation of state roots.

The job ran on `ghr-euw-10`, an AMD EPYC 4585PX 16-Core host, with rustc 1.98.1. All twelve measured launches specify CPUs 1–15 and a 127,918,040,268-byte memory limit. These are launch limits, not an independent affinity or memory-use measurement. All twelve startup logs explicitly load `StorageSettings { storage_v2: true }`; source dispatch selects packed trie keys. The recorded snapshot-manifest hash prefix is `1de6440dc1acb2a5`, not a full snapshot checksum. Workflow, summary script and production metric-config hashes match between baseline and candidate.

Equal-run mean sampled peak RSS is 10,479.20→10,475.20 MiB (−4.00 MiB); peak jemalloc allocation is 3,978.11→3,975.36 MiB (−2.75 MiB). Peak active and allocator-resident changes are −2.97/−2.89 MiB. Last available RSS/allocated values change −3.84/−3.63 MiB. These descriptive results show no material footprint change.

Each process has 131–134 in-window RSS samples roughly 200 ms apart. The last raw gauge sample is 8.596–8.860 seconds before the target-metrics window ends. Values are sampled maxima and last available observations, not true peaks, final-window values or post-drain measurements. RSS includes resident mapped pages and overlaps allocator accounting; neither is total host/cgroup memory. Do not add them or infer page-cache use from their difference.

No source, workflow, benchmark threshold or production metric configuration was changed for this audit. This directory preserves compact summaries, all official tails, per-process endpoints and hashes; large raw artifacts remain in the local benchmark workspace.
