# DST campaign bug ledger

This ledger tracks distinct reproducible product failures, not each failing seed. For each
failure record its first seed, replay trace, invariant, minimal reproducer, and disposition.
Inconclusive cases (watchdog, resource exhaustion, unsupported native durability claims) are
tracked separately and never counted toward a bug-free qualification interval.

| First seed | Replay trace | Distinct invariant failure | Reproducer / disposition |
| --- | --- | --- | --- |
| None confirmed | — | — | Campaign not yet qualified. |

## Observed status anomalies (not yet classified as product defects)

| First seed | Trace | Observation | Follow-up |
| --- | --- | --- | --- |
| 36 | `target/reth-dst/failures/node-36-75be66d2.dst` | A fault injected into a follower's `get-encoded HashedAccounts` read preceded an Engine `Invalid` response (`links to previously rejected block`, `latestValidHash` = genesis) for the modeled chain. | The fault-aware harness now cold-restarts before retrying; seed 36 passes with strict replay after that change. Investigate whether Engine should report an internal error instead of `Invalid` during the transient read failure before declaring a product defect. |

## Harness / branch blockers

| Base revision | Reproducer | Failure | Disposition |
| --- | --- | --- | --- |
| `5fbd646697` | `cargo check -p reth-dst-runner --features dst --bin reth-dst-node` | The supplied branch did not compile: missing Engine message fields and type/async/handoff mismatches in engine-tree (28 diagnostics before reaching the runner). | Fixed in this branch; type-check passed. This is a build blocker, not a confirmed runtime invariant failure. |
| `5fbd646697` + first generator extension | Seed 0; trace `target/reth-dst/failures/node-0-33bddf9b.dst` | An empty payload build waited for a txpool snapshot despite having no senders to prewarm. | Fixed the harness wait condition. The input is valid; this was a harness false positive, not a product invariant failure. |
| First snapshot oracle | Seeds 0, 1, 2, 4; traces in `target/reth-dst/failures/` | The original snapshot compared storage `None` to `Some(0)` as different despite both reading as zero in Ethereum, and used bytecode `Debug` with an internal lazy hash cache. | Normalize absent storage to zero and compare original code bytes; the 12-seed matrix now passes strict replay. |
| First 1900-second campaign | Seed 302; `target/reth-dst/failures/node-302-fccce47a.dst` | A modeled forkchoice changed heads while a payload build was pending; the old job returned `MissingPayload` on resolution, which the harness unconditionally unwrapped. | Strictly replayed the trace; recognize the canceled build only after an observed head change, discard its transactions, and preserve the existing success assertion for uninterrupted jobs. This was a harness lifecycle error, not a confirmed engine defect. Restart the clean-run clock on the rebuilt binary. |

After the canceled-build fix, seeds 298–309 passed with strict semantic replay, including seed
302 and injected cursor, transaction-open, and write failures.
| `5fbd646697` engine-tree suite | `cargo test -p reth-engine-tree --features dst --lib` | Four deterministic fixtures configured a persistence threshold before reducing the default state-masking window, violating a builder invariant. | Fixed the fixtures by clearing the masking window first; not a product-state failure. |

## Inconclusive test-suite observations

| Test | Observation | Follow-up |
| --- | --- | --- |
| `persistence::tests::test_read_only_consistency_across_reorg` | The complete engine-tree suite fails to acquire an MDBX lock (`Resource temporarily unavailable`) even when run serially; the focused test passes. Its multi-open debug mode must be configured before any MDBX environment opens in the process. | 238 of 239 tests pass serially after the deterministic fixture corrections. This does not establish a reorg-state mismatch; the test requires an isolated process or a reliable process-wide multi-open setup. |
| `tree::payload_processor::prewarm::tests::deterministic_block_prewarm_cache_lifecycle` | The parallel suite once missed a speculative-cache hit; seed 0 passes in isolation. | Re-run serially and with individual seeds; do not classify as a unique product defect without a stable replay. |

## Qualification

The first 1,900-second attempt started at 2026-10-05 06:25:26 UTC on the pre-cancellation build
and was stopped after seed 302 exposed the canceled-payload harness error. A replacement attempt
started at 2026-10-05 06:42:28 UTC with seed 318, 16 workload steps per case, and a 90-second host
case watchdog. Its end time, case count, and result must be filled in only after it actually runs
for more than 30 minutes without a distinct new failure.

Start the 30-minute clock only after a successful build and after the last **new distinct**
invariant failure. Record wall-clock start and end, build revision, campaign options, case count,
number of inconclusive cases, and all distinct bugs. A clean interval requires more than 30 minutes
of actual running time; simulated time and compile time do not count. The deterministic executor
does not establish native thread-preemption, weak-memory, OS-crash, mmap/writeback, or power-loss
durability guarantees; these require separately recorded native/host fault experiments.
