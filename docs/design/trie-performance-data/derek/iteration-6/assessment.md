# Derek 6: retained component improvements; overall result neutral

Run [33870980404](https://github.com/paradigmxyz/reth/actions/runs/33870980404) compares original baseline `eadb887f1c1698e7bd932f60a0c654dccccd081f` with `a0d8b420cd949b41cf93893300b494aa35e57d47`. The candidate combines the retained trie changes, fixed-B256 leaf sorting, and borrowed cursor-operation metrics. It retains FIFO reclamation, WRITEMAP, prefault writes, and the original durable synchronization settings. Main merge `2042e313a2b7cb0e7606dceeca75005612702d8f` has the same source as the tested candidate.

The run used six independent processes per variant, 500 measured mainnet blocks after 125 warmups, metrics enabled and profiling disabled. The unchanged Derek run-cluster bootstrap and practical floors remain authoritative. These components are retained based on component benchmarks and neutral overall behavior; **the requested overall improvement remains unqualified**.

| Official headline | Candidate change | Reported 95% half-width | Verdict |
| --- | ---: | ---: | --- |
| mean | +0.0940% | 0.4556% | neutral |
| p50 | +0.6179% | 0.6858% | neutral |
| p90 | +0.3231% | 0.9783% | neutral |
| p99 | +0.0596% | 1.2088% | neutral |
| mgas_s | -0.1424% | 0.3198% | neutral |
| wall_clock | +0.0642% | 0.4679% | neutral |
| persist_wait | -3.0533% | 1.0744% | neutral |

Mean newPayload latency is 23.8432→23.8656 ms. The reported wall-clock sum is 72.0216→72.0679 seconds across 3,000 rows per variant; it is not elapsed replay time. Persistence wait falls 26.410→25.604 ms (3.05%), below the unchanged 5% floor. It measures engine backpressure with `wait-for-persistence=never`, not a final durable drain.

Supplementary actual per-process replay duration, from `report.run_stats.duration_ms`, averages 26.608→26.191 seconds (−1.566% ±0.447%, the same bootstrap primitive). This is a separate diagnostic and does not replace the official neutral headline verdict.

## Root-task phases and worker attribution

Values below are cumulative histogram seconds divided by canonical-height delta, averaged equally across runs. Negative changes mean faster. The diagnostic 5% floors do not alter production configuration or official acceptance.

| Phase | Baseline ms/block | Candidate ms/block | Change | Reported 95% half-width |
| --- | ---: | ---: | ---: | ---: |
| final_update_duration_histogram | 0.337100 | 0.305512 | -9.3704% | 3.2394% |
| process_updates_duration_histogram | 8.699031 | 8.404706 | -3.3834% | 0.6922% |
| total_duration_histogram | 23.041326 | 23.017934 | -0.1015% | 0.3163% |
| reveal_multiproof_duration_histogram | 4.446077 | 4.502256 | +1.2636% | 1.3464% |
| channel_wait_duration_histogram | 4.985997 | 5.175688 | +3.8045% | 1.3651% |

Final-root work improves 9.37% ±3.24%, while the total root-task duration remains neutral. Processing updates saves 0.294 ms/block; channel waiting adds 0.190 ms and revealing proofs adds 0.056 ms. These observations describe elapsed phases under overlap, not pure CPU work or proof that one change caused another.

The official `hash-post-state` worker duration improves 12.93% ±4.83%, from 314.524 to 273.868 µs/block. Its queue wait changes from 17.870 to 19.827 µs and is neutral. Source inspection shows the sparse-mode worker normally blocks receiving already hashed state; it does not execute the modified trie sort. Its elapsed duration includes that receive wait. Faster sparse-loop progress could make the data available sooner, but this metric does not measure faster hashing or sorting CPU, nor isolate the sorting component from other retained changes. See `worker-source-audit.md`.

Across all twelve raw archives, process-update calls increase 84.84→89.31 per canonical block (+5.27%), and reveal calls 36.16→37.55 (+3.85%). Account/storage cache-miss totals increase 6.26%/5.40%; hits change +0.69%/+0.03%. All four cache histogram observation counts agree within each process; they are recorded per completed task. Hits/misses count offered updates across passes, including retries, and each process call can visit many storage tries. These aggregates do not reconstruct individual sort sizes, unique keys, or why scheduling differed. No sort-batch-size histogram is exported; `num_updates` is a trace-span field.

## Direct persistence and completed work

| Phase | Baseline ms/canonical block | Candidate ms/canonical block | Change | Reported 95% half-width |
| --- | ---: | ---: | ---: | ---: |
| write_trie_updates | 19.426182 | 18.866599 | -2.8806% | 0.9379% |
| total | 28.348329 | 27.733881 | -2.1675% | 0.8570% |
| commit_mdbx | 15.052275 | 14.862085 | -1.2635% | 2.1787% |
| write_hashed_state | 8.234608 | 8.171682 | -0.7642% | 1.1004% |
| write_state | 0.617909 | 0.626284 | +1.3555% | 9.3422% |
| insert_block | 0.031339 | 0.031731 | +1.2527% | 8.2844% |
| update_history_indices | 0.000353 | 0.000337 | -4.5390% | 3.6128% |
| update_pipeline_stages | 0.003188 | 0.003122 | -2.0878% | 2.5841% |

The direct trie-persistence phase includes waiting for deferred trie data, merging updates, and node writes. The hashed-state phase also includes collecting references and merging; write_state includes plain state and enabled changesets/receipts. Save-total excludes the later commit. These values must not be described as isolated database writes or added to overlapping root/worker phases.

The windows complete 3,024 baseline blocks in 378 batches versus 3,008 candidate blocks in 376 batches; every batch contains eight blocks. All twelve strict append-log/counter checks agree. There are no missing work metrics, negative deltas, or trie-call/batch-count mismatches. Candidate processes 3 and 4 include 496 persisted blocks; the others and all baselines include 504. The sampled canonical-height deltas are 497–499, rather than exactly 500, because the scrape endpoints differ from payload endpoints.

Normalizing each process by its completed persisted blocks gives trie-persistence service time 19.214→18.729 ms/block, −2.526% ±0.797%. This removes the different block counts but not differences in the particular boundary blocks. It does not establish an improvement beyond the 5% diagnostic floor or a durable-throughput result.

MDBX commit remains neutral. Across all read-write commits, the supplementary commit-stage audit shows whole time 15.031→14.842 ms/canonical block, synchronization 14.948→14.760 ms, and GC wall time 0.07949→0.07836 ms. Explicit-write time is reported zero in both WRITEMAP variants; this does not mean no I/O. All eight commit-component counts agree within each process (124–126 completed commits). These counters also include other writers such as pruning. GC CPU time overlaps GC wall time and must not be added to it.

## Workload identity and memory

All twelve reports and received-block logs match the same 500-block ordered payload, heights 25,883,613–25,884,112, 150,862 transactions, and 15,020,694,260 gas. The payload SHA-256 is `70c2694e3941f5de87c466907b13f2f3bbb93d4f5bc1bcbb6455485600573c50`, also matching the earlier iteration-2 payload. Report source labels match the official baseline/candidate SHAs. This audit checks reported/logged hashes and work; it does not independently recompute state roots.

Mean sampled peak RSS changes 10,483.69→10,453.86 MiB (−29.84 MiB, −0.285%); peak jemalloc allocation 3,980.29→3,977.42 MiB (−2.87 MiB, −0.072%). Peak active allocation changes −3.37 MiB and allocator resident memory +5.12 MiB. Last available RSS changes −32.46 MiB and allocated memory +9.66 MiB. These descriptive differences do not indicate a material footprint change.

There are 131–135 in-window RSS samples per process, roughly 200 ms apart. The last available raw gauge sample occurs 8.59–8.84 seconds before the target-metrics window ends. Consequently these are sampled maxima and last available values, not true peak, final-window, or post-drain memory. RSS includes mapped file pages and overlaps jemalloc accounting; allocator retained memory is address space, not necessarily resident memory. Do not add the metrics or infer total host/cgroup/page-cache use. MDBX residency-probe/page-operation metrics are not part of the captured diagnostic schema; missing metrics are not zero.

No benchmark thresholds, production metrics configuration, source, or workflows were changed for this analysis. Raw archives stay in the local benchmark workspace; this directory contains compact summaries, per-process endpoints, and provenance hashes.
