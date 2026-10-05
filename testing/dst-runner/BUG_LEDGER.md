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
| `5fbd646697` and first PR CI run | GitHub Actions doctests on PR #27733 | Cargo could not fetch private `tempoxyz/abi-fuzz` (HTTP 401) on CI runners; the only manifest reference was an unused dependency of `reth-dst-runner`. | Removed its workspace and runner declarations and lock entry; head checkout now resolves locally without it. The compact-codec CI job explicitly checks out the unchanged base branch for comparison and still fails with the same 401; that base-side CI blocker cannot be fixed by a head-only change. No runtime behavior changed. |
| `5fbd646697` and head-only workspace CI | `cargo test --no-run --workspace --exclude ef-tests --features asm-keccak --locked` | The custom-engine-types example did not forward the new `state_provider_factory` field in `BuildArguments`, producing E0027/E0063. | Forwarded that field in the example; `cargo check --locked -p example-custom-engine-types` passes. The next CI run still needs verification; this is unrelated to node-campaign runtime behavior. |
| `5fbd646697` + first generator extension | Seed 0; trace `target/reth-dst/failures/node-0-33bddf9b.dst` | An empty payload build waited for a txpool snapshot despite having no senders to prewarm. | Fixed the harness wait condition. The input is valid; this was a harness false positive, not a product invariant failure. |
| First snapshot oracle | Seeds 0, 1, 2, 4; traces in `target/reth-dst/failures/` | The original snapshot compared storage `None` to `Some(0)` as different despite both reading as zero in Ethereum, and used bytecode `Debug` with an internal lazy hash cache. | Normalize absent storage to zero and compare original code bytes; the 12-seed matrix now passes strict replay. |
| First 1900-second campaign | Seed 302; `target/reth-dst/failures/node-302-fccce47a.dst` | A modeled forkchoice changed heads while a payload build was pending; the old job returned `MissingPayload` on resolution, which the harness unconditionally unwrapped. | Strictly replayed the trace; recognize the canceled build only after an observed head change, discard its transactions, and preserve the existing success assertion for uninterrupted jobs. This was a harness lifecycle error, not a confirmed engine defect. Restart the clean-run clock on the rebuilt binary. |
| Native-worker smoke after qualification | Seeds 318–325, first trace `target/reth-dst/failures/node-318-e2e08ad2.dst` | All eight cases hit the same `validation must preserve its sparse trie` harness assertion. | The native path selected a serial state-root fallback that does not preserve a sparse-trie frontier. Force the native differential lane to exercise parallel state-root workers even under a CPU quota; seed 318 and a subsequent eight-seed smoke pass. This was one harness configuration mismatch, not eight product defects. |

After the canceled-build fix, seeds 298–309 passed with strict semantic replay, including seed
302 and injected cursor, transaction-open, and write failures.
| `5fbd646697` engine-tree suite | `cargo test -p reth-engine-tree --features dst --lib` | Four deterministic fixtures configured a persistence threshold before reducing the default state-masking window, violating a builder invariant. | Fixed the fixtures by clearing the masking window first; not a product-state failure. |

## Inconclusive test-suite observations

| Test | Observation | Follow-up |
| --- | --- | --- |
| `persistence::tests::test_read_only_consistency_across_reorg` | The complete engine-tree suite failed to acquire an MDBX lock (`Resource temporarily unavailable`) even when run serially; the focused test passed. Its multi-open debug mode must be configured before any MDBX environment opens in the process. | Run that test in an isolated child process before any environment opens. The focused test and all 239 engine-tree tests pass serially after isolation. This was a test-process setup failure, not a reorg-state mismatch. |
| `tree::payload_processor::prewarm::tests::deterministic_block_prewarm_cache_lifecycle` | The parallel suite once missed a speculative-cache hit; seed 0 passes in isolation. | Re-run serially and with individual seeds; do not classify as a unique product defect without a stable replay. |

## Qualification

The first 1,900-second attempt started at 2026-10-05 06:25:26 UTC on the pre-cancellation build
and was stopped after seed 302 exposed the canceled-payload harness error. The replacement
campaign ran from 2026-10-05 06:42:28 UTC to its observed completion at 07:14:14 UTC on
2026-10-05: **31 minutes 46 seconds of wall-clock observation**, exceeding the 30-minute
threshold. It used the debug `reth-dst-node` binary built from `23482a267e`, seed 318,
`RETH_DST_SECONDS=1900`, `RETH_DST_STEPS=16`, and a 90-second host case watchdog. Seeds 318–882
completed: **565 cases, zero distinct new invariant failures, zero inconclusive cases, exit code
0**. The subsequent changes removed an unused dependency, changed example and test fixtures,
and forced parallel state roots **only in the separate native-worker profile**; the cooperative
runtime branch qualified above is unchanged. Before this
clean interval, seed 302 reproduced a harness cancellation error; see the ledger above. No
product-state invariant defect was confirmed during this run.

Start the 30-minute clock only after a successful build and after the last **new distinct**
invariant failure. Record wall-clock start and end, build revision, campaign options, case count,
number of inconclusive cases, and all distinct bugs. A clean interval requires more than 30 minutes
of actual running time; simulated time and compile time do not count. The deterministic executor
does not establish native thread-preemption, weak-memory, OS-crash, mmap/writeback, or power-loss
durability guarantees; these require separately recorded native/host fault experiments.
