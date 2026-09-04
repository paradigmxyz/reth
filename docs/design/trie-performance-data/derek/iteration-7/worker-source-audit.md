# Derek 7 attribution boundaries

Candidate `8c2ab90521c046f00a1f20b56adabe623c1e3fce` changes eligibility in `compute_drained_storage_roots`: it iterates pending account updates and requires their storage-update sets to be drained. The retained root/hash and database changes remain present; the identical-leaf experiment is excluded. The official comparison is against the fixed original baseline, not against iteration 6 in the same six-pair experiment.

The helper runs from `promote_pending_account_updates`, after the existing empty-pending-account early return. `make_progress` includes this promotion work in `sparse_trie_process_updates_duration_histogram` when update/proof queues permit it. The other recorded processing path calls `process_new_updates` without promotion. Therefore process duration is a relevant containing phase, not a direct timer for the eligibility check or storage hashing alone.

Actual scheduled storage-root count is the debug-span field `n` on `compute_drained_storage_roots`, not an exported histogram. No dedicated metric measures the number of roots avoided by this filter. Cache hits/misses and process/reveal call counts can describe aggregate scheduling/workload differences, but cannot establish skipped-root incidence or pure CPU savings.

As in iteration 6, the `hash-post-state` named worker normally receives already hashed state from the sparse loop. Its elapsed duration includes the blocking receive. Final-root timing measures work remaining after incremental processing; total sparse-task time includes channel waiting and overlaps execution. Phase differences must not be summed across overlapping task boundaries or treated as proof of causality.

The phase, completed-persistence, payload/source and memory audits retain their previous definitions. The original official bootstrap, practical floors and verdicts remain authoritative. Candidate adoption stays undecided until results are available; no gain is assumed from source inspection.
