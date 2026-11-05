//! Replays historical mainnet blocks from a read-only datadir against in-memory parent state.
//!
//! Each block's parent-state reads are recorded once from the datadir's history, then every timed
//! run executes the block against that in-memory copy. This isolates the engine thread's
//! execution cost (EVM, per-transaction commit and receipts) from state access, and lets
//! execution strategies be compared on identical inputs.

#![warn(unused_crate_dependencies)]

use alloy_consensus::BlockHeader;
use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap},
    Address, Bytes, B256, U256,
};
use clap::Parser;
use evm2::{
    bytecode::Bytecode,
    evm::{AccountInfo, Database},
};
use reth_ethereum::{
    chainspec::MAINNET,
    node::EthereumNode,
    provider::{
        providers::{BlockchainProvider, ReadOnlyConfig},
        BlockReader, StateProviderFactory, TransactionVariant,
    },
};
use reth_evm::{
    database::StateProviderDatabase,
    execute::{BlockExecutor, BlockExecutorFactory},
    precompile_cache::PrecompileCacheMap,
    ConfigureEvm, SpecFor,
};
use reth_evm_ethereum::EthEvmConfig;
use reth_primitives_traits::RecoveredBlock;
use reth_provider::BlockExecutionOutput;
use reth_storage_api::EvmStateProviderAdapter;
use serde::Serialize;
use std::{
    fs::File,
    io::{BufWriter, Write},
    path::PathBuf,
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Debug, Parser)]
struct Args {
    /// Path to a mainnet datadir with state history.
    #[arg(long)]
    datadir: String,
    /// First block to replay.
    #[arg(long)]
    from: u64,
    /// Number of blocks to replay.
    #[arg(long, default_value_t = 100)]
    count: u64,
    /// Timed runs per block.
    #[arg(long, default_value_t = 3)]
    runs: usize,
    /// Stream per-transaction state updates to a consumer thread, as the engine does for the
    /// sparse trie.
    #[arg(long)]
    hook: bool,
    /// Execute without the precompile cache. By default the cache is warmed by the recording
    /// pass, as prewarming warms it before the engine executes a block.
    #[arg(long)]
    no_precompile_cache: bool,
    /// Writes each replayed block with its recorded parent-state reads to this file, one JSON
    /// [`BlockCase`] per line.
    #[arg(long)]
    dump: Option<PathBuf>,
}

fn main() -> eyre::Result<()> {
    let args = Args::parse();
    let spec = MAINNET.clone();
    let runtime = reth_ethereum::tasks::Runtime::test();
    let factory = EthereumNode::provider_factory_builder().open_read_only(
        spec.clone(),
        ReadOnlyConfig::from_datadir(&args.datadir),
        runtime,
    )?;
    let provider = BlockchainProvider::new(factory.clone())?;
    let evm_config = EthEvmConfig::new(spec);
    let precompiles = (!args.no_precompile_cache).then(PrecompileCacheMap::default);

    let mut dump = args.dump.as_ref().map(File::create).transpose()?.map(BufWriter::new);
    let mut total = Totals::default();
    for number in args.from..args.from + args.count {
        let block = factory
            .recovered_block(number.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| eyre::eyre!("block {number} not found"))?;

        let mut mem = MemDb::default();
        let state = provider.history_by_block_number(number - 1)?;
        let expected = {
            let db = Recorder {
                inner: StateProviderDatabase::new(EvmStateProviderAdapter(state)),
                mem: &mut mem,
            };
            execute_sequential(&evm_config, precompiles.clone(), db, &block, false)?.0
        };

        // Timed runs execute on their own named thread so that profiles can select them.
        let best = std::thread::scope(|scope| {
            std::thread::Builder::new()
                .name("timed".to_string())
                .spawn_scoped(scope, || {
                    let mut best = Duration::MAX;
                    for _ in 0..args.runs {
                        let (output, elapsed) = execute_sequential(
                            &evm_config,
                            precompiles.clone(),
                            &mem,
                            &block,
                            args.hook,
                        )?;
                        eyre::ensure!(
                            output.result.receipts == expected.result.receipts,
                            "receipts differ"
                        );
                        eyre::ensure!(output.state == expected.state, "state differs");
                        best = best.min(elapsed);
                    }
                    eyre::Ok(best)
                })?
                .join()
                .map_err(|_| eyre::eyre!("timed thread panicked"))?
        })?;

        if let Some(dump) = &mut dump {
            serde_json::to_writer(&mut *dump, &BlockCase::new(&block, &mem))?;
            dump.write_all(b"\n")?;
        }
        total.record(&block, best, &mem);
        println!(
            "block={number} txs={} gas={} reads(acc={} slot={}) seq={:.3}ms",
            block.body().transactions.len(),
            block.gas_used(),
            mem.accounts.len(),
            mem.storage.len(),
            best.as_secs_f64() * 1e3,
        );
    }
    if let Some(mut dump) = dump {
        dump.flush()?;
    }
    total.print();
    Ok(())
}

/// Executes `block` transaction by transaction, the way the engine does, and returns the output
/// with the time spent from pre-execution changes to the finished output.
fn execute_sequential<DB: Database>(
    evm_config: &EthEvmConfig,
    precompiles: Option<PrecompileCacheMap<SpecFor<EthEvmConfig>>>,
    db: DB,
    block: &RecoveredBlock<reth_ethereum::Block>,
    hook: bool,
) -> eyre::Result<(BlockExecutionOutput<reth_ethereum::Receipt>, Duration)> {
    let (updates_tx, updates_rx) = mpsc::channel();
    let consumer = hook.then(|| std::thread::spawn(move || updates_rx.into_iter().count()));

    let start = Instant::now();
    let evm_env = evm_config.evm_env(block.header())?;
    let evm = match precompiles {
        Some(precompiles) => evm_config.evm_with_precompile_cache(db, evm_env, precompiles, false),
        None => evm_config.evm_with_env(db, evm_env),
    };
    let ctx = evm_config.context_for_block(block.sealed_block())?;
    let mut executor = evm_config.block_executor_factory().create_executor(evm, ctx);
    if consumer.is_some() {
        executor.set_state_hook(move |update| {
            let _ = updates_tx.send(update);
        });
    }
    executor.apply_pre_execution_changes()?;
    for tx in block.transactions_recovered() {
        executor.execute_transaction(tx)?;
    }
    let output = executor.finish()?;
    let elapsed = start.elapsed();

    if let Some(consumer) = consumer {
        let _ = consumer.join();
    }
    Ok((output, elapsed))
}

/// Accumulated replay statistics.
#[derive(Debug, Default)]
struct Totals {
    blocks: u64,
    txs: u64,
    gas: u64,
    seq: Duration,
    latencies: Vec<f64>,
}

impl Totals {
    fn record(&mut self, block: &RecoveredBlock<reth_ethereum::Block>, seq: Duration, _: &MemDb) {
        self.blocks += 1;
        self.txs += block.body().transactions.len() as u64;
        self.gas += block.gas_used();
        self.seq += seq;
        self.latencies.push(seq.as_secs_f64() * 1e3);
    }

    fn print(&mut self) {
        self.latencies.sort_by(f64::total_cmp);
        let pct = |p: f64| self.latencies[((self.latencies.len() - 1) as f64 * p) as usize];
        println!(
            "blocks={} txs={} gas={} seq_total={:.1}ms mean={:.3}ms p50={:.3}ms p99={:.3}ms ggas/s={:.3}",
            self.blocks,
            self.txs,
            self.gas,
            self.seq.as_secs_f64() * 1e3,
            self.seq.as_secs_f64() * 1e3 / self.blocks as f64,
            pct(0.5),
            pct(0.99),
            self.gas as f64 / self.seq.as_secs_f64() / 1e9,
        );
    }
}

/// A block with every parent-state value its execution reads.
#[derive(Debug, Serialize)]
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
#[derive(Debug, Serialize)]
struct CaseAccount {
    balance: U256,
    nonce: u64,
    code_hash: B256,
}

impl BlockCase {
    fn new(block: &RecoveredBlock<reth_ethereum::Block>, mem: &MemDb) -> Self {
        Self {
            block: alloy_rlp::encode(block.clone_block()).into(),
            senders: block.senders().to_vec(),
            accounts: mem
                .accounts
                .iter()
                .map(|(address, info)| {
                    let account = info.as_ref().map(|info| CaseAccount {
                        balance: info.balance,
                        nonce: info.nonce,
                        code_hash: info.code_hash,
                    });
                    (*address, account)
                })
                .collect(),
            storage: mem
                .storage
                .iter()
                .map(|(&(address, key), &value)| (address, key, value))
                .collect(),
            code: mem.code.iter().map(|(hash, code)| (*hash, code.original_bytes())).collect(),
            block_hashes: mem.block_hashes.iter().map(|(number, hash)| (*number, *hash)).collect(),
        }
    }
}

/// Error returned when an execution reads state outside the recorded parent-state reads.
#[derive(Debug)]
struct MissingState;

impl std::fmt::Display for MissingState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("state was not recorded")
    }
}

impl std::error::Error for MissingState {}

/// Parent state recorded for one block.
#[derive(Debug, Default)]
struct MemDb {
    accounts: AddressMap<Option<AccountInfo>>,
    storage: HashMap<(Address, U256), U256>,
    code: B256Map<Bytecode>,
    block_hashes: HashMap<U256, B256>,
}

impl Database for &MemDb {
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

/// Database that records every read of `inner` into a [`MemDb`].
struct Recorder<'a, DB> {
    inner: DB,
    mem: &'a mut MemDb,
}

impl<DB: Database> Database for Recorder<'_, DB> {
    type Error = DB::Error;

    fn get_account(&mut self, address: &Address) -> Result<Option<AccountInfo>, Self::Error> {
        let account = self.inner.get_account(address)?;
        self.mem.accounts.insert(*address, account.clone());
        Ok(account)
    }

    fn get_code_by_hash(&mut self, code_hash: &B256) -> Result<Bytecode, Self::Error> {
        let code = self.inner.get_code_by_hash(code_hash)?;
        self.mem.code.insert(*code_hash, code.clone());
        Ok(code)
    }

    fn get_storage(&mut self, address: &Address, key: &U256) -> Result<U256, Self::Error> {
        let value = self.inner.get_storage(address, key)?;
        self.mem.storage.insert((*address, *key), value);
        Ok(value)
    }

    fn get_block_hash(&mut self, number: &U256) -> Result<B256, Self::Error> {
        let hash = self.inner.get_block_hash(number)?;
        self.mem.block_hashes.insert(*number, hash);
        Ok(hash)
    }
}
