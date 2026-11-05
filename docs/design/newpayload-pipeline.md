# newPayload pipeline: parallel execution with an ordered committer

Status: in progress. Phase 1 is on branch `mattsse/parallel-newpayload`
([paradigmxyz/reth#28293](https://github.com/paradigmxyz/reth/pull/28293)).

## Goal

Validate an Ethereum mainnet payload about three times faster than reth `main`: from about
1.1 Ggas/s and a 25-34 ms mean `engine_newPayload` (derek, 500 mainnet blocks) to about 3 Ggas/s
and 10 ms. Nethermind is 20-30% faster than reth on mainnet today, so tuning the existing
sequential path is not enough.

## Where the time goes

Measured on recent mainnet blocks (26,327,216-26,327,715, about 28 Mgas and 340 transactions per
block) with `examples/exec-replay`. It records every parent-state value a block reads from a
read-only archive datadir and replays the block in memory, so execution can be timed and compared
without state access.

| Measurement | Mean per block |
|---|---|
| derek `newPayload`, reth `main` | 25-34 ms |
| Sequential execution in memory, precompile cache warm | 9.4-9.7 ms (2.9-3.3 Ggas/s) |
| Sequential execution in memory, precompile cache cold | 12.1 ms |
| Ordered commit of ready speculative results, in memory | 1.2 ms |

Two conclusions:

1. Even with every account and slot in memory, executing a mainnet block sequentially takes
   about 10 ms. A `perf` profile of that run puts 56% in the evm2 interpreter's opcode handlers,
   and the rest in state bookkeeping, hashing and commit (`BlockState` transition maps, the evm2
   overlay, keccak). Removing overhead around the interpreter cannot reach 3x, so the engine
   thread must stop executing most transactions itself.
2. The other 15-25 ms of `newPayload` are state access on the engine thread (cross-block cache
   lookups and MDBX reads that prewarming did not get to first), the state root after the last
   transaction, and fixed costs. Parallel execution also removes the state reads from the
   engine thread, because speculative workers do them.

## Phase 1: speculative parallel execution with an ordered committer

reth already speculates: prewarm workers execute every transaction against the parent state plus
the writes earlier speculative results predict, and record their reads. Block execution can commit
a result instead of executing the transaction when its reads still match the committed state,
rebasing balances the transaction only credited or debited (`payload_processor/reuse.rs`,
[#27894](https://github.com/paradigmxyz/reth/pull/27894)). On mainnet it gained about 7%.

### The rules are not the limit

The corpus test `corpus_reuse` speculates every transaction of a block in order, refreshes stale
results, then runs the commit loop:

| Mode, 100 blocks, 42,702 transactions | Reused | Commit loop |
|---|---|---|
| Speculation against the parent only | 64.6% | 5.9 ms |
| Plus one in-order refresh against predicted writes | 99.6% | 1.2 ms |
| Sequential execution | - | 9.4 ms |

Receipts and bundle state are identical in every mode. With ready results the engine thread does
7-8x less work.

### The scheduling was the limit

- Prewarming spawned one task per transaction and refreshed stale results only after all first
  executions finished.
- Refreshing polled with 50 us sleeps.
- Block execution never waited. It reached transactions whose results were still in flight,
  executed them itself with cold state, and fell behind the workers for the rest of the block.

The new scheduler (`prewarm.rs`, `speculate_txs`):

- Every prewarm pool thread runs one worker for the whole block. A worker claims the next queued
  transaction in block order. When none is queued, it executes again the first stale result among
  the 64 transactions after the one block execution is at. It rescans that window only after the
  committer moved or a result was stored.
- The committer (the engine thread) waits for a transaction whose first speculative execution is
  in flight. It executes a transaction itself only if no worker started it or its result is
  stale, and publishes the actual writes so that later results are refreshed against them.

### Hot keys

A profile of the committer under 16 workers had 14% in `dashmap` exclusive-lock contention: nearly
every transaction credits the block beneficiary, so every speculative result published a predicted
balance for one key, and every rebased reuse republished it. Predictions only steer speculation.
Correctness comes from validating reads against the committed state, so leaving a write out of
the predictions is always safe. Results no longer predict the beneficiary's balance-only writes,
and speculation sees the beneficiary as non-empty so its credit is rebased. Nonce changes are
still predicted, because builders send transactions from the beneficiary within the block, which
made 95% of stale results when everything about the beneficiary was left out. Reuse also
republishes only rebased account infos, not a result's whole write set.

### Results so far

In memory with real worker threads (`corpus_parallel`), all 500 corpus blocks (170,510
transactions), 8 workers, identical receipts and bundle state for every block:

| | Mean | p50 | p99 |
|---|---|---|---|
| Sequential | 10.5 ms | | |
| Parallel | 5.8 ms | 5.2 ms | 14.8 ms |

Worker counts on 200 blocks: 4 workers 6.2 ms, 6 and 8 workers 5.7 ms, 12 workers 6.2 ms. The
committer's 5.8 ms split into 1.7-1.8 ms waiting for first speculations, 2.4 ms executing the
27 stale results per block itself (mostly swaps on pools an earlier transaction in the block also
used), and about 1.6 ms of commit work. The stale ones form dependency chains that are serial
whoever executes them: waiting once for an in-flight refresh instead of executing cut the
committer's own executions to 1.3 ms but added the same time in waiting. So this design's
in-memory floor is about 1.75x for execution. Going lower needs cheaper chain links (a JIT, or a
faster interpreter) or merging the effects of common conflicts, as balance rebasing already does
for balances.

derek on mainnet, first run (idle workers spinning): mean 32.2 -> 38.0 ms (+18%), Mgas/s
1,183 -> 1,028. The in-memory runs had nothing else competing for the cores. In the node, every
prewarm thread spun for the whole block, taking CPU time from the sparse trie and proof workers
that compute the state root at the same time; the persistence wait rose from 0.06 to 0.78 ms.
Idle workers now wait on a condition variable. Second run: mean 31.9 -> 32.3 ms (+1.0%, within
the ±1.0% interval), Mgas/s +0.7%, end-to-end Mgas/s -3.3% (the persistence wait still rose).

So the regression is gone, but the execution phase that is 1.8x faster in memory does not shorten
`newPayload` on mainnet. Either the engine thread is not the critical path there (the state root
pipeline, or fixed costs), or speculation in the node is slow enough that the committer waits for
it (its state reads are cold cache and MDBX reads, unlike in memory). derek with `--debug.skip-state-root` on both sides separates the two:

| 500 blocks, 3 run pairs | main | branch | Change |
|---|---|---|---|
| Mean | 25.98 ms | 24.94 ms | -4.0% |
| P50 | 18.18 ms | 16.52 ms | -9.2% |
| Mgas/s | 1,508 | 1,660 | +10.1% |

So on mainnet the parallel execution does shorten the execution phase, and the state root then
absorbs the gain: main itself spends about 6 ms of its 32 ms mean on the root beyond execution,
and the branch finishes execution earlier only to wait longer for it. (A third arm, main with
only reuse enabled, failed twice while building on derek's runner.) The state root is now the
critical path, and it is the next phase.

## Phase 2: the state root as the critical path

How the root is computed today (`state_root_strategy/sparse_trie.rs`, `trie/sparse`):

- Each transaction's `StateUpdate` goes to one "trie-hashing" thread, which hashes addresses and
  slots serially, then to the "sparse-trie" thread, which applies leaves in batches. A leaf under
  a blinded node waits for a proof from the proof worker pools.
- Prewarm hints only prefetch proofs. They carry no values, so no leaf is applied before block
  execution commits its transaction.
- After the last transaction, the sparse-trie thread drains outstanding proof rounds (several
  round trips after removals that collapse onto blinded siblings), then serially returns the
  storage tries, hashes the upper account trie and collects trie updates. Only subtrie hashing
  and large storage passes use rayon.
- While updates keep arriving, `make_progress` applies buffered updates, runs storage passes and
  promotes account updates when the update queue is empty and no proof results are pending, and
  otherwise every `UPDATE_WINDOW_LEN` (16) update messages or after `UPDATE_WINDOW_INTERVAL`
  (1 ms). Before the window it did so only once, for the first 64 messages, so a producer that
  streams an update per transaction without gaps pushed nearly all trie work past the last
  transaction. In tempo's bench-e2e the builder's state-root wait is a fixed ~28-30 ms p50
  regardless of block size, and prefetch hints cut only ~3 ms of it (tempoxyz/tempo#8979).
- `tree.root.sparse_trie_finish_to_root_duration_histogram` measures the time from the finish
  marker to the root, and `tree.root.sparse_trie_proof_rounds_after_finish_histogram` the proof
  target batches dispatched after it. Both are bench target metrics.

On derek's mainnet blocks the tail is not where the root's cost is (paradigmxyz/reth#28297):

- Without the window, finish to root is 1.91 ms per block with 0.79 proof rounds after the marker.
  A window of 16 messages cuts that to 1.77 ms and 0.54 rounds; windows of 64 and 256 do not
  change it measurably. None of them moves newPayload latency.
- `--debug.skip-state-root` saves about 6 ms of newPayload mean on main (32.0 to 26.1 ms), but the
  post-execution root wait is only 1.2 ms on main and 2.2 ms with Phase 1. The rest is contention
  while execution runs: with the root on, the engine thread on Phase 1 sees 40 involuntary context
  switches per block instead of 7, and `prewarm-txs` takes 24% longer than on main. Phase 1's
  gain goes into that contention, not into a deferred tail.

Speculation already knows about 99% of the block's final writes within the first few
milliseconds, long before block execution commits them. The plan:

1. Done: measure the tail on derek, see above.
2. Done: bounded trie work while updates arrive, as zempo's `root_ahead` applies a window of
   changes beside execution. Leaving gaps in the stream from the producer side does not do it in
   tempo: its builder sending updates in bursts of 32 (tempoxyz/tempo#8984) moved its finish p50
   only from 27.6 to 26.9 ms, so the task itself schedules the work. Whether the window shortens
   tempo's ~28 ms wait is not measured yet.
3. Root ahead of commit: feed the sparse trie the predicted values of stored speculative results,
   so that proofs are fetched and storage tries hashed while block execution is still running.
   When block execution commits a transaction, only leaves whose committed value differs from the
   applied prediction are rewritten. The sparse trie already tolerates rewriting a leaf many
   times per block, so this changes when leaves arrive, not what the root is.
4. Parallel finish: hash addresses on the workers that produced the results instead of one
   thread, and collect storage trie updates in parallel.

The other critical-path items, once the root no longer hides execution:

| Component | Evidence | Plan |
|---|---|---|
| Commit loop | 12% read-set validation, 17% `PredictedState::update`, 4% `BlockState::commit_account_changes` on the committer, in memory | Cheaper prediction storage (an arena per block instead of a `DashMap` of sorted vectors). Validating against predictions instead of the state was tried and is unsound: predictions omit some lifecycle changes, and an existing test caught it |
| Dependency chains | 27 stale results per block, 2.4 ms of serial re-execution in memory | Merge the effects of common conflicts the way balances are rebased; a faster interpreter or JIT for chain links |
| Execution cache inserts | Tempo profile: `ExecutionCache::insert_state` 5.2% of the engine thread | Move inserts off the engine thread, back the tables with huge pages |
| Proof worker pools | A tempo node logs "Spawning proof worker pools" with 32 storage and 32 account workers for every block on 16 threads | Start the pools once and size them to the cores |
| Keccak cache | Tempo profile: sparse-trie hashing thrashes the global keccak cache, so the EVM's mapping-slot hashes miss | Bypass the global cache for trie hashing |

## Safety

The new path is behind `--engine.prewarm-reuse` (default on in the branch, `=false` restores
sequential execution).

- `reuse::tests::fuzz_reuse_matches_sequential_execution` generates blocks of transactions that
  depend on each other through balances, nonces, storage, the beneficiary and destroyed accounts,
  speculates them in a random order with and without beneficiary predictions, and asserts the
  receipts, bundle state and streamed state updates of sequential execution. 512 cases run in CI;
  a local run of 50,000 passed. It fails if result validation is skipped.
- `corpus_reuse` and `corpus_parallel` replay recorded mainnet blocks and assert identical receipts
  and bundle state for every mode. Corpus files are written by `exec-replay --dump` and are not
  checked in (3.6 GB for 500 blocks).
- `e2e::prewarm_reuse` has a follower validate blocks of dependent transactions with reuse. It
  fails if result validation is skipped.
- `prewarm::tests::speculation_queue_claims_arrived_transactions_in_order` covers the queue.

Canary to add before enabling by default: a shadow mode that executes a sampled block
sequentially off the critical path and compares receipts and bundle state, recording a metric
and an error log on mismatch.

## Measuring

```bash
# record and replay blocks from a read-only archive datadir
example-exec-replay --datadir <datadir> --from <block> --count 500 --runs 3 --dump corpus.jsonl
# compare execution modes on the corpus
RETH_BLOCK_CORPUS=corpus.jsonl RETH_PARALLEL_WORKERS=8 \
  cargo test --profile profiling -p reth-engine-tree --lib corpus_parallel -- --ignored --nocapture
```
