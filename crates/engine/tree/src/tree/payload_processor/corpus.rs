//! Replays of recorded mainnet blocks against their recorded parent-state reads.
//!
//! A corpus is a file of JSON lines, each a block with every parent-state value its execution
//! reads, as written by the `exec-replay` example's `--dump`. The tests here are ignored and read
//! the corpus from `RETH_BLOCK_CORPUS`.

use super::reuse::{
    BalanceTracker, PrewarmResults, RecordingDb, RecordingHandle, ReuseMiss, SpeculativeTx,
};
use alloy_consensus::{BlockHeader, Transaction};
use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap},
    Address, Bytes, B256, U256,
};
use alloy_rlp::Decodable;
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
};
use reth_chainspec::MAINNET;
use reth_ethereum_primitives::{Block, Receipt};
use reth_evm::{
    execute::{BlockExecutor, BlockExecutorFactory},
    precompile_cache::PrecompileCacheMap,
    ConfigureEvm, Evm, EvmEnv, EvmTypesFor, SpecFor,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::{RecoveredBlock, SealedBlock};
use reth_trie_common::bundle_state::BundleState;
use serde::Deserialize;
use std::{
    io::BufRead,
    sync::Arc,
    time::{Duration, Instant},
};

/// A block with every parent-state value its execution reads.
#[derive(Debug, Deserialize)]
struct BlockCase {
    /// RLP encoding of the block.
    block: Bytes,
    senders: Vec<Address>,
    /// Accounts by address, `None` for accounts that do not exist.
    accounts: Vec<(Address, Option<CaseAccount>)>,
    storage: Vec<(Address, U256, U256)>,
    /// Original bytecode by code hash.
    code: Vec<(B256, Bytes)>,
    block_hashes: Vec<(U256, B256)>,
}

/// Account fields of a [`BlockCase`].
#[derive(Debug, Deserialize)]
struct CaseAccount {
    balance: U256,
    nonce: u64,
    code_hash: B256,
}

/// Parent state of a [`BlockCase`].
#[derive(Debug, Default)]
struct CaseDb {
    accounts: AddressMap<Option<AccountInfo>>,
    storage: HashMap<(Address, U256), U256>,
    code: B256Map<Bytecode>,
    block_hashes: HashMap<U256, B256>,
}

/// Error returned when an execution reads state the corpus did not record.
#[derive(Debug, thiserror::Error)]
#[error("state was not recorded")]
struct MissingState;

impl Database for &CaseDb {
    type Error = MissingState;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        self.accounts.get(address).cloned().ok_or(MissingState)
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        self.code.get(code_hash).cloned().ok_or(MissingState)
    }

    fn get_storage(&mut self, address: &Address, key: &U256) -> Result<U256, Self::Error> {
        self.storage.get(&(*address, *key)).copied().ok_or(MissingState)
    }

    fn get_block_hash(&mut self, number: &U256) -> Result<B256, Self::Error> {
        self.block_hashes.get(number).copied().ok_or(MissingState)
    }
}

impl BlockCase {
    fn into_parts(self) -> (RecoveredBlock<Block>, CaseDb) {
        let block = Block::decode(&mut self.block.as_ref()).expect("valid block RLP");
        let block = RecoveredBlock::new_sealed(SealedBlock::seal_slow(block), self.senders);
        let db = CaseDb {
            accounts: self
                .accounts
                .into_iter()
                .map(|(address, account)| {
                    let info = account.map(|account| AccountInfo {
                        balance: account.balance,
                        nonce: account.nonce,
                        code_hash: account.code_hash,
                        ..Default::default()
                    });
                    (address, info)
                })
                .collect(),
            storage: self
                .storage
                .into_iter()
                .map(|(address, key, value)| ((address, key), value))
                .collect(),
            code: self
                .code
                .into_iter()
                .map(|(hash, code)| (hash, Bytecode::new_raw(code)))
                .collect(),
            block_hashes: self.block_hashes.into_iter().collect(),
        };
        (block, db)
    }
}

/// Reads the corpus at `RETH_BLOCK_CORPUS`, calling `f` with each block and its parent state.
fn for_each_case(mut f: impl FnMut(RecoveredBlock<Block>, CaseDb)) {
    let path = std::env::var("RETH_BLOCK_CORPUS").expect("RETH_BLOCK_CORPUS is set");
    let limit = std::env::var("RETH_BLOCK_CORPUS_LIMIT")
        .ok()
        .map_or(usize::MAX, |limit| limit.parse().expect("valid limit"));
    let file = std::io::BufReader::new(std::fs::File::open(path).expect("corpus exists"));
    for line in file.lines().take(limit) {
        let case: BlockCase = serde_json::from_str(&line.expect("readable corpus")).unwrap();
        let (block, db) = case.into_parts();
        f(block, db);
    }
}

/// Output of a block execution that replays compare.
#[derive(Debug, PartialEq, Eq)]
struct Output {
    receipts: Vec<Receipt>,
    bundle: BundleState,
}

/// Executes `block` sequentially, as the engine does without prewarm results.
fn execute_sequential(
    config: &EthEvmConfig,
    precompiles: &PrecompileCacheMap<SpecFor<EthEvmConfig>>,
    db: &CaseDb,
    block: &RecoveredBlock<Block>,
) -> (Output, Duration) {
    let start = Instant::now();
    let env = config.evm_env(block.header()).unwrap();
    let evm = config.evm_with_precompile_cache(db, env, precompiles.clone(), false);
    let ctx = config.context_for_block(block.sealed_block()).unwrap();
    let mut executor = config.block_executor_factory().create_executor(evm, ctx);
    executor.apply_pre_execution_changes().unwrap();
    for tx in block.transactions_recovered() {
        executor.execute_transaction(tx).unwrap();
    }
    let output = executor.finish().unwrap();
    (Output { receipts: output.result.receipts, bundle: output.state }, start.elapsed())
}

/// Statistics of [`execute_with_reuse`].
#[derive(Debug, Default)]
struct ReuseStats {
    reused: usize,
    missing: usize,
    stale: usize,
    /// Time of the speculative executions, summed over all of them.
    speculation: Duration,
    /// Time of the in-order commit loop that reuses results or executes transactions.
    commit: Duration,
}

/// Speculatively executes every transaction of `block` against the parent state, as if all ran
/// at once, refreshes stale results in block order, then executes the block reusing the results.
fn execute_with_reuse(
    config: &EthEvmConfig,
    precompiles: &PrecompileCacheMap<SpecFor<EthEvmConfig>>,
    db: &CaseDb,
    block: &RecoveredBlock<Block>,
    refresh: bool,
) -> (Output, ReuseStats) {
    let mut stats = ReuseStats::default();
    let txs = block.transactions_recovered().map(|tx| tx.cloned()).collect::<Vec<_>>();
    let results = if std::env::var("RETH_UNPREDICTED_BENEFICIARY").is_ok() {
        PrewarmResults::with_unpredicted_account(txs.len(), block.header().beneficiary())
    } else {
        PrewarmResults::<EvmTypesFor<EthEvmConfig>>::new(txs.len())
    };
    let explain = std::env::var("RETH_EXPLAIN_STALE").is_ok();

    let env = config.evm_env(block.header()).unwrap();
    let recording = RecordingHandle::default();
    let mut speculative_evm = config.evm_with_precompile_cache(
        RecordingDb::new(db, recording.clone()),
        env.clone().with_nonce_check_disabled().with_balance_check_disabled(),
        precompiles.clone(),
        false,
    );
    let mut speculate = |index: usize, predicted: bool| {
        let tx = &txs[index];
        let tx_env = config.tx_env(tx.clone());
        let speculative = SpeculativeTx::new(tx.signer(), tx.inner());
        drop(speculative_evm.take_state_cache());
        recording.start(predicted.then(|| (Arc::clone(results.predicted()), index)));
        let result = Evm::transact_with_inspector(
            &mut speculative_evm,
            &tx_env,
            BalanceTracker::new(recording.clone()),
        );
        let reads = recording.take_reads().unwrap();
        match result {
            Ok((mut balances, result)) => {
                results.store(index, result, reads, &mut balances, speculative)
            }
            Err(_) => results.reject(index, reads),
        }
    };

    let start = Instant::now();
    for index in 0..txs.len() {
        speculate(index, false);
    }
    if refresh {
        for index in 0..txs.len() {
            if results.needs_refresh(index) {
                speculate(index, true);
            }
        }
    }
    stats.speculation = start.elapsed();

    let start = Instant::now();
    let evm = config.evm_with_precompile_cache(db, env, precompiles.clone(), false);
    let ctx = config.context_for_block(block.sealed_block()).unwrap();
    let mut executor = config.block_executor_factory().create_executor(evm, ctx);
    executor.apply_pre_execution_changes().unwrap();
    for (index, tx) in txs.iter().enumerate() {
        executor.validate_transaction_gas_limit(tx.inner().gas_limit()).unwrap();
        let outcome = results.take_valid(index, executor.evm_mut().state_db_mut()).unwrap();
        match outcome {
            Ok(result) => {
                let result = config.prewarm_tx_result(tx.inner(), result).unwrap();
                executor.commit_transaction(result).unwrap();
                stats.reused += 1;
            }
            Err(miss) => {
                if explain {
                    println!(
                        "stale block={} index={index} beneficiary={} {}",
                        block.number(),
                        block.header().beneficiary(),
                        results.explain(index, executor.evm_mut().state_db_mut()),
                    );
                }
                let result = executor.execute_transaction_without_commit(tx.clone()).unwrap();
                results.publish_executed(index, &result.result().pending_state);
                executor.commit_transaction(result).unwrap();
                match miss {
                    ReuseMiss::Missing => stats.missing += 1,
                    _ => stats.stale += 1,
                }
            }
        }
    }
    let output = executor.finish().unwrap();
    stats.commit = start.elapsed();
    (Output { receipts: output.result.receipts, bundle: output.state }, stats)
}

/// Reports how many transactions of the corpus blocks reuse speculative results, and how long
/// the in-order commit loop takes compared to sequential execution.
#[test]
#[ignore = "reads a block corpus from RETH_BLOCK_CORPUS"]
fn corpus_reuse() {
    let config = EthEvmConfig::new(MAINNET.clone());
    let precompiles = PrecompileCacheMap::default();
    let refresh = std::env::var("RETH_BLOCK_CORPUS_NO_REFRESH").is_err();
    let (mut blocks, mut txs, mut gas) = (0usize, 0usize, 0u64);
    let mut totals = ReuseStats::default();
    let mut sequential = Duration::ZERO;
    for_each_case(|block, db| {
        // Warm the precompile cache, as prewarming does before the engine executes a block.
        let (expected, _) = execute_sequential(&config, &precompiles, &db, &block);
        let (output, seq) = execute_sequential(&config, &precompiles, &db, &block);
        assert_eq!(output, expected, "block {}", block.number());
        let (output, stats) = execute_with_reuse(&config, &precompiles, &db, &block, refresh);
        assert_eq!(output, expected, "block {} with reuse", block.number());

        println!(
            "block={} txs={} seq={:.3}ms commit={:.3}ms speculation={:.3}ms reused={} missing={} stale={}",
            block.number(),
            block.body().transactions.len(),
            seq.as_secs_f64() * 1e3,
            stats.commit.as_secs_f64() * 1e3,
            stats.speculation.as_secs_f64() * 1e3,
            stats.reused,
            stats.missing,
            stats.stale,
        );
        blocks += 1;
        txs += block.body().transactions.len();
        gas += block.gas_used();
        sequential += seq;
        totals.reused += stats.reused;
        totals.missing += stats.missing;
        totals.stale += stats.stale;
        totals.speculation += stats.speculation;
        totals.commit += stats.commit;
    });
    println!(
        "blocks={blocks} txs={txs} reused={:.1}% missing={} stale={} seq_mean={:.3}ms commit_mean={:.3}ms speculation_mean={:.3}ms seq_ggas/s={:.3} commit_ggas/s={:.3}",
        totals.reused as f64 * 100.0 / txs as f64,
        totals.missing,
        totals.stale,
        sequential.as_secs_f64() * 1e3 / blocks as f64,
        totals.commit.as_secs_f64() * 1e3 / blocks as f64,
        totals.speculation.as_secs_f64() * 1e3 / blocks as f64,
        gas as f64 / sequential.as_secs_f64() / 1e9,
        gas as f64 / totals.commit.as_secs_f64() / 1e9,
    );
}

#[global_allocator]
static ALLOC: tikv_jemallocator::Jemalloc = tikv_jemallocator::Jemalloc;

/// Number of transactions ahead of the committer that idle workers refresh.
const REFRESH_WINDOW: usize = 64;

/// Statistics of [`execute_parallel`].
#[derive(Debug, Default)]
struct ParallelStats {
    reused: usize,
    /// Transactions the committer executed because no worker had started them.
    unclaimed: usize,
    /// Transactions the committer executed because their results were stale.
    stale: usize,
    /// Speculative executions, including refreshes.
    speculations: usize,
    /// Time the committer spent waiting for in-flight speculative results.
    waiting: Duration,
    /// Time the committer spent executing transactions itself.
    executing: Duration,
    /// Time from the start of speculation to the finished output.
    wall: Duration,
}

/// Executes `block` with `pool`'s threads speculating transactions in block order and refreshing
/// stale results ahead of the committer, which commits valid results in order and waits for
/// results that are in flight instead of executing those transactions itself.
fn execute_parallel(
    config: &EthEvmConfig,
    precompiles: &PrecompileCacheMap<SpecFor<EthEvmConfig>>,
    db: &CaseDb,
    block: &RecoveredBlock<Block>,
    pool: &rayon::ThreadPool,
) -> (Output, ParallelStats) {
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

    let start = Instant::now();
    let txs = block.transactions_recovered().map(|tx| tx.cloned()).collect::<Vec<_>>();
    let tx_count = txs.len();
    let results = if std::env::var("RETH_PREDICT_BENEFICIARY").is_ok() {
        PrewarmResults::<EvmTypesFor<EthEvmConfig>>::new(tx_count)
    } else {
        PrewarmResults::with_unpredicted_account(tx_count, block.header().beneficiary())
    };
    let env = config.evm_env(block.header()).unwrap();
    let next = AtomicUsize::new(0);
    let committed = AtomicUsize::new(0);
    // A worker holds a transaction's claim while it checks or speculates it, and marks it as
    // speculating only while it executes, which is what the committer waits for.
    let claimed = (0..tx_count).map(|_| AtomicBool::new(false)).collect::<Vec<_>>();
    let speculating = (0..tx_count).map(|_| AtomicBool::new(false)).collect::<Vec<_>>();
    let done = AtomicBool::new(false);
    let speculations = AtomicUsize::new(0);

    let worker = || {
        let recording = RecordingHandle::default();
        let mut evm = config.evm_with_precompile_cache(
            RecordingDb::new(db, recording.clone()),
            env.clone().with_nonce_check_disabled().with_balance_check_disabled(),
            precompiles.clone(),
            false,
        );
        let mut speculate = |index: usize| {
            speculating[index].store(true, Ordering::Release);
            let tx = &txs[index];
            let tx_env = config.tx_env(tx.clone());
            let speculative = SpeculativeTx::new(tx.signer(), tx.inner());
            drop(evm.take_state_cache());
            recording.start(Some((Arc::clone(results.predicted()), index)));
            let result = Evm::transact_with_inspector(
                &mut evm,
                &tx_env,
                BalanceTracker::new(recording.clone()),
            );
            let reads = recording.take_reads().unwrap();
            match result {
                Ok((mut balances, result)) => {
                    results.store(index, result, reads, &mut balances, speculative)
                }
                Err(_) => results.reject(index, reads),
            }
            speculating[index].store(false, Ordering::Release);
            speculations.fetch_add(1, Ordering::Relaxed);
        };
        // Committer position and number of stored speculations at the last refresh scan that
        // found nothing, so that idle workers rescan only after something changed.
        let mut scanned = (usize::MAX, usize::MAX);
        let mut idle = 0u32;
        while !done.load(Ordering::Acquire) {
            let index = if next.load(Ordering::Relaxed) < tx_count {
                next.fetch_add(1, Ordering::Relaxed)
            } else {
                tx_count
            };
            if index < tx_count {
                speculating[index].store(true, Ordering::Release);
                if index >= committed.load(Ordering::Relaxed) {
                    speculate(index);
                }
                speculating[index].store(false, Ordering::Release);
                continue
            }
            // The committer executes the transaction it is at itself if its result is stale.
            let from = committed.load(Ordering::Relaxed) + 1;
            let state =
                (from, speculations.load(Ordering::Relaxed) + results.predicted().generation());
            if state == scanned {
                backoff(&mut idle);
                continue
            }
            idle = 0;
            let mut refreshed = false;
            for index in from..(from + REFRESH_WINDOW).min(tx_count) {
                if claimed[index].swap(true, Ordering::AcqRel) {
                    continue
                }
                if index >= committed.load(Ordering::Relaxed) && results.needs_refresh(index) {
                    speculate(index);
                    refreshed = true;
                }
                claimed[index].store(false, Ordering::Release);
                if refreshed {
                    break
                }
            }
            if !refreshed {
                scanned = state;
            }
        }
    };

    let mut stats = ParallelStats::default();
    let output = pool.in_place_scope(|scope| {
        for _ in 0..pool.current_num_threads() {
            scope.spawn(|_| worker());
        }
        let evm = config.evm_with_precompile_cache(db, env.clone(), precompiles.clone(), false);
        let ctx = config.context_for_block(block.sealed_block()).unwrap();
        let mut executor = config.block_executor_factory().create_executor(evm, ctx);
        executor.apply_pre_execution_changes().unwrap();
        for (index, tx) in txs.iter().enumerate() {
            executor.validate_transaction_gas_limit(tx.inner().gas_limit()).unwrap();
            let mut wait_start = None;
            loop {
                match results.take_valid(index, executor.evm_mut().state_db_mut()).unwrap() {
                    Ok(result) => {
                        let result = config.prewarm_tx_result(tx.inner(), result).unwrap();
                        executor.commit_transaction(result).unwrap();
                        stats.reused += 1;
                        break
                    }
                    Err(miss) => {
                        // Only first speculations are waited for: a refresh may chase predictions
                        // that keep changing, so the committer executes stale results itself.
                        // Only first speculations are waited for: a refresh may chase predictions
                        // that keep changing, so the committer executes stale results itself.
                        let in_flight = miss == ReuseMiss::Missing &&
                            (speculating[index].load(Ordering::Acquire) ||
                                index < next.load(Ordering::Relaxed));
                        if in_flight {
                            wait_start.get_or_insert_with(Instant::now);
                            std::hint::spin_loop();
                            continue
                        }
                        let execute_start = Instant::now();
                        let result =
                            executor.execute_transaction_without_commit(tx.clone()).unwrap();
                        results.publish_executed(index, &result.result().pending_state);
                        executor.commit_transaction(result).unwrap();
                        stats.executing += execute_start.elapsed();
                        match miss {
                            ReuseMiss::Missing => stats.unclaimed += 1,
                            ReuseMiss::Stale => stats.stale += 1,
                        }
                        break
                    }
                }
            }
            if let Some(wait_start) = wait_start {
                stats.waiting += wait_start.elapsed();
            }
            committed.store(index + 1, Ordering::Relaxed);
        }
        done.store(true, Ordering::Release);
        executor.finish().unwrap()
    });
    stats.wall = start.elapsed();
    stats.speculations = speculations.into_inner();
    (Output { receipts: output.result.receipts, bundle: output.state }, stats)
}

/// Spins, then yields, for longer the more consecutive idle rounds `idle` counts.
fn backoff(idle: &mut u32) {
    if *idle < 6 {
        for _ in 0..1 << *idle {
            std::hint::spin_loop();
        }
    } else {
        std::thread::yield_now();
    }
    *idle = idle.saturating_add(1);
}

/// Reports the wall-clock time of [`execute_parallel`] against sequential execution, with
/// `RETH_PARALLEL_WORKERS` worker threads.
#[test]
#[ignore = "reads a block corpus from RETH_BLOCK_CORPUS"]
fn corpus_parallel() {
    let config = EthEvmConfig::new(MAINNET.clone());
    let precompiles = PrecompileCacheMap::default();
    let workers = std::env::var("RETH_PARALLEL_WORKERS")
        .ok()
        .map_or(16, |workers| workers.parse().expect("valid worker count"));
    let pool = rayon::ThreadPoolBuilder::new()
        .num_threads(workers)
        .thread_name(|index| format!("speculate-{index}"))
        .build()
        .unwrap();
    let (mut blocks, mut txs, mut gas) = (0usize, 0usize, 0u64);
    let mut totals = ParallelStats::default();
    let mut sequential = Duration::ZERO;
    let mut latencies = Vec::new();
    for_each_case(|block, db| {
        let (expected, _) = execute_sequential(&config, &precompiles, &db, &block);
        let (_, seq) = execute_sequential(&config, &precompiles, &db, &block);
        // The committer runs on its own named thread so that profiles can select it.
        let (output, stats) = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("committer".to_string())
                .spawn_scoped(scope, || execute_parallel(&config, &precompiles, &db, &block, &pool))
                .unwrap()
                .join()
                .unwrap()
        });
        assert_eq!(output, expected, "block {} in parallel", block.number());

        println!(
            "block={} txs={} seq={:.3}ms par={:.3}ms wait={:.3}ms reused={} unclaimed={} stale={} speculations={}",
            block.number(),
            block.body().transactions.len(),
            seq.as_secs_f64() * 1e3,
            stats.wall.as_secs_f64() * 1e3,
            stats.waiting.as_secs_f64() * 1e3,
            stats.reused,
            stats.unclaimed,
            stats.stale,
            stats.speculations,
        );
        blocks += 1;
        txs += block.body().transactions.len();
        gas += block.gas_used();
        sequential += seq;
        latencies.push(stats.wall.as_secs_f64() * 1e3);
        totals.reused += stats.reused;
        totals.unclaimed += stats.unclaimed;
        totals.stale += stats.stale;
        totals.speculations += stats.speculations;
        totals.waiting += stats.waiting;
        totals.executing += stats.executing;
        totals.wall += stats.wall;
    });
    latencies.sort_by(f64::total_cmp);
    println!(
        "workers={workers} blocks={blocks} txs={txs} reused={:.1}% unclaimed={} stale={} speculations/tx={:.2} seq_mean={:.3}ms par_mean={:.3}ms par_p50={:.3}ms par_p99={:.3}ms wait_mean={:.3}ms self_exec_mean={:.3}ms seq_ggas/s={:.3} par_ggas/s={:.3}",
        totals.reused as f64 * 100.0 / txs as f64,
        totals.unclaimed,
        totals.stale,
        totals.speculations as f64 / txs as f64,
        sequential.as_secs_f64() * 1e3 / blocks as f64,
        totals.wall.as_secs_f64() * 1e3 / blocks as f64,
        latencies[latencies.len() / 2],
        latencies[(latencies.len() - 1) * 99 / 100],
        totals.waiting.as_secs_f64() * 1e3 / blocks as f64,
        totals.executing.as_secs_f64() * 1e3 / blocks as f64,
        gas as f64 / sequential.as_secs_f64() / 1e9,
        gas as f64 / totals.wall.as_secs_f64() / 1e9,
    );
}
