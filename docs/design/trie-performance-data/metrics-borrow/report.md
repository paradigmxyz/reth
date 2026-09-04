Cursor metrics borrowing experiment, 2026-09-04

Candidate `0798ad069ca8566be6a783b2acebf7ad9504b321` removes an Arc clone/drop from each measured MDBX cursor write by borrowing the metrics and cursor fields separately. Metrics remain enabled with the same operation counts and duration boundaries. Baseline is `33c491663d96c545e49e8af4cb6edc233132f0ce`.

The bounded experiment supports a small improvement in the measured write phase: metrics-enabled paired median changes are -1.03%, -1.96%, -2.23%, and -1.24%. After dividing by the matching metrics-disabled control ratio, the changes are -0.92%, -0.42%, -1.62%, and -1.04%. It does not demonstrate faster total durable persistence. The component was included in the next combined Derek experiment for workload qualification; these local results alone do not qualify the overall persistence goal.

Method and correctness

Two frozen executables use Rust 1.98.1 (`48a229cea`), the profiling profile, and `RUSTFLAGS="-C target-cpu=native"`. The baseline was compiled with the exact retained-33 cursor.rs restored in the same diagnostic worktree, retaining an identical harness and feature graph. The added regression test is cfg(test) only. Changed DB/provider/trie-db packages were explicitly cleaned before compiling, and the harness asserts effective FIFO and WRITEMAP flags. Binary and diagnostic-patch SHA-256 hashes are recorded in metadata.json.

Eight processes ran ABBA twice, pinned to CPU 0, with other local builds and performance jobs held. Each process alternated metrics-on/off order by round on independent temporary databases, discarded four warmups, and recorded 40 samples per mode: 640 measured rows. Each persistent environment starts with 32,768 packed account and 65,536 packed storage branch records; each sample replaces 4,096 account and 8,192 storage records through the real writer, then commits with SyncMode::Durable.

Metrics-on uses real atomic Counter handles. Every enabled sample asserts exactly 20,480 cursor operations; disabled samples assert zero. Histograms are noop in this harness because all operation values are below the 4,096-byte duration threshold. Separate regression tests cover large-value histogram recording, successful writes and duplicate-insert errors, all six write operations, exact counts, and finite nonnegative durations. No timing assertion depends on sleeping or a positive elapsed duration.

All samples passed account/storage snapshot equality after commit and final close/reopen. These untimed full reads warm all live pages between updates; this is a warm persistent-environment fixture, not a cold workload. Durable commit and reopen checks establish logical persistence, not crash or power-loss recovery. The exact production commit passed all 56 focused DB/trie tests, nightly formatting, and diff checks.

Paired results

Each percentage is 100 × (candidate / baseline - 1), so negative means faster. Pair order follows the ABBA sequence. Write and durable columns compare medians within each process; the final column compares means.

| Baseline/candidate runs | Write, metrics on | Write, metrics off | Write on/off normalized | Durable, metrics on | Durable mean, metrics on |
|---|---:|---:|---:|---:|---:|
| 1 / 2 | -1.029% | -0.110% | -0.920% | +1.195% | +1.386% |
| 4 / 3 | -1.962% | -1.551% | -0.418% | +0.328% | +5.794% |
| 5 / 6 | -2.229% | -0.614% | -1.625% | -0.376% | -0.377% |
| 8 / 7 | -1.236% | -0.202% | -1.036% | -0.101% | -0.419% |

The metrics-off control also improves modestly, so the unadjusted metrics-on difference should not all be attributed to Arc removal. Normalization is a descriptive control comparison, not an independently estimated confidence interval.

Commit latency changed substantially during the capture: early process medians were about 44–45 ms versus about 27 ms in the later processes. Baseline run 4 straddled that change. Its paired durable mean increase was +5.79% with metrics on and +6.60% with metrics off, even though their median changes were only +0.33% and +0.58%. Consequently neither neutral median comparisons nor pooled distributions establish a durable throughput improvement. No confidence interval or 5% production qualification is claimed from four pairs.

Pooled sample distributions are retained for transparency (milliseconds, median [Q1, Q3]; quartiles use inclusive interpolation). Pooled durable medians are especially sensitive to the commit-latency mixture and should not replace the paired comparisons above.

| Mode | Phase | Baseline | Candidate |
|---|---|---:|---:|
| Metrics on | Write | 13.612 [13.501, 13.723] | 13.374 [13.235, 13.472] |
| Metrics off | Write | 13.185 [13.110, 13.257] | 13.083 [13.026, 13.179] |
| Metrics on | Durable | 40.764 [40.282, 57.744] | 47.508 [40.222, 58.332] |
| Metrics off | Durable | 40.235 [39.948, 57.655] | 46.190 [39.990, 58.186] |

Reproduction and artifacts

Apply diagnostic-harness.patch on the production candidate for the benchmark target, its metrics dev dependency, and the matching lockfile entry. No diagnostic dependency or harness is part of the production commit. For a baseline binary, restore cursor.rs exactly from the baseline revision before compiling the same harness; restore the candidate source afterwards. Use a separate target directory or explicitly invalidate changed package artifacts.

```bash
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/tmp/reth-metrics-borrow-native-target RUSTFLAGS="-C target-cpu=native" cargo +1.98.1 nextest run --locked --cargo-profile profiling -p reth-db -p reth-trie-db --lib --test storage_persistence --test sparse_root_persistence
LLVM_SYS_221_PREFIX=/usr/lib/llvm-22 CARGO_BUILD_JOBS=8 CARGO_TARGET_DIR=/tmp/reth-metrics-borrow-native-target RUSTFLAGS="-C target-cpu=native" cargo +1.98.1 bench --locked --profile profiling -p reth-trie-db --bench cursor_metrics --no-run
MPT_METRICS_SAMPLES=40 taskset -c 0 ./cursor-metrics-baseline > run-1-baseline.csv 2> run-1-baseline.log
# Run candidate, candidate, baseline, baseline, candidate, candidate, baseline,
# retaining separate CSV/log files and keeping the local timing slot quiet.
python3 summarize.py
```

metadata.json contains exact source revisions, compiler, binary hashes and fixture preconditions; runs.json contains process order/timestamps/exit codes. run-*.csv preserve all samples. summary.json records per-process and pooled medians, quartiles, means, and paired changes. tests.log and build-*.log preserve validation and build evidence. The benchmark does not disable production metrics, change public APIs, alter durability, or modify vendored MDBX code.
