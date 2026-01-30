//! Benchmark tool for comparing MDBX vs RocksDB historical query performance.
//!
//! Supports two benchmark types:
//! 1. `changeset` - Historical state queries (eth_getBalance, eth_getTransactionCount, eth_getStorageAt)
//! 2. `txhash` - TxHashNumbers table lookups (TxHash -> TxNumber)
//!
//! Three cache modes:
//! - AllCold: Clear OS cache + reopen DB before each query
//! - JarCached: Keep DB open, clear OS cache before each query
//! - AllWarm: Keep DB open, no cache clearing

use alloy_primitives::{Address, BlockNumber, TxHash, TxNumber, B256};
use clap::Parser;
use eyre::Result;
use rand::{seq::SliceRandom, Rng};
use reth_db_api::{cursor::DbCursorRO, models::StorageSettings, tables, transaction::DbTx};
use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    node::EthereumNode,
    provider::{
        providers::ReadOnlyConfig, AccountReader, BlockNumReader, ChangeSetReader, StateProvider,
        StorageChangeSetReader,
    },
};
use reth_provider::providers::{HistoricalStateProviderRef, HistoryInfo};
use reth_storage_api::{BlockBodyIndicesProvider, StorageSettingsCache, TransactionsProvider};
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    fs::File,
    io::{BufReader, BufWriter, Write},
    path::PathBuf,
    process::Command,
    time::{Duration, Instant},
};

#[derive(Parser, Debug)]
#[command(name = "changeset-bench")]
#[command(about = "Benchmark MDBX vs RocksDB historical query performance")]
struct Args {
    /// Path to the datadir (used for both backends when bench-type is txhash)
    #[arg(long)]
    datadir: Option<PathBuf>,

    /// Path to the normal (MDBX) datadir (for changeset benchmarks)
    #[arg(long)]
    normal: Option<PathBuf>,

    /// Path to the edge (RocksDB/SF) datadir (for changeset benchmarks)
    #[arg(long)]
    edge: Option<PathBuf>,

    /// Number of queries to run per method
    #[arg(long, default_value = "500")]
    queries: usize,

    /// Number of samples (should be >= queries for accurate results)
    #[arg(long, default_value = "500")]
    samples: usize,

    /// Benchmark mode
    #[arg(long, value_enum, default_value = "all")]
    mode: BenchMode,

    /// Benchmark type
    #[arg(long, value_enum, default_value = "changeset")]
    bench_type: BenchType,

    /// Path to save/load sampled data
    #[arg(long)]
    sample_file: Option<PathBuf>,

    /// Profile mode: run detailed timing breakdown
    #[arg(long)]
    profile: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum BenchMode {
    All,
    AllCold,
    JarCached,
    AllWarm,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum BenchType {
    Changeset,
    Txhash,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountSample {
    address: Address,
    first_change: BlockNumber,
    last_change: BlockNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StorageSample {
    address: Address,
    key: B256,
    first_change: BlockNumber,
    last_change: BlockNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TxHashSample {
    hash: TxHash,
    tx_number: TxNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SampledData {
    accounts: Vec<AccountSample>,
    storage: Vec<StorageSample>,
    tx_hashes: Vec<TxHashSample>,
    oldest_block: BlockNumber,
    latest_block: BlockNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchmarkQuery {
    query_type: QueryType,
    address: Address,
    storage_key: Option<B256>,
    block: BlockNumber,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct TxHashQuery {
    hash: TxHash,
    expected_tx_number: TxNumber,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
enum QueryType {
    Balance,
    Nonce,
    Storage,
}

#[derive(Debug, Default)]
struct BenchmarkResults {
    timings: Vec<Duration>,
}

struct ModeResults {
    balance: (BenchmarkResults, BenchmarkResults),
    nonce: (BenchmarkResults, BenchmarkResults),
    storage: (BenchmarkResults, BenchmarkResults),
}

struct TxHashModeResults {
    mdbx: BenchmarkResults,
    rocksdb: BenchmarkResults,
}

#[derive(Debug, Default)]
struct HistoryStats {
    in_changeset: usize,
    in_plain_state: usize,
    not_yet_written: usize,
    maybe_in_plain_state: usize,
}

impl HistoryStats {
    fn record(&mut self, info: &HistoryInfo) {
        match info {
            HistoryInfo::InChangeset(_) => self.in_changeset += 1,
            HistoryInfo::InPlainState => self.in_plain_state += 1,
            HistoryInfo::NotYetWritten => self.not_yet_written += 1,
            HistoryInfo::MaybeInPlainState => self.maybe_in_plain_state += 1,
        }
    }

    fn total(&self) -> usize {
        self.in_changeset + self.in_plain_state + self.not_yet_written + self.maybe_in_plain_state
    }

    fn print(&self, label: &str) {
        let total = self.total();
        if total == 0 {
            println!("  {}: no queries", label);
            return;
        }
        println!("  {} History Distribution ({} queries):", label, total);
        println!(
            "    InChangeset:       {:>5} ({:>5.1}%)",
            self.in_changeset,
            self.in_changeset as f64 / total as f64 * 100.0
        );
        println!(
            "    InPlainState:      {:>5} ({:>5.1}%)",
            self.in_plain_state,
            self.in_plain_state as f64 / total as f64 * 100.0
        );
        println!(
            "    NotYetWritten:     {:>5} ({:>5.1}%)",
            self.not_yet_written,
            self.not_yet_written as f64 / total as f64 * 100.0
        );
        println!(
            "    MaybeInPlainState: {:>5} ({:>5.1}%)",
            self.maybe_in_plain_state,
            self.maybe_in_plain_state as f64 / total as f64 * 100.0
        );
    }
}

impl BenchmarkResults {
    fn new() -> Self {
        Self { timings: Vec::new() }
    }

    fn add(&mut self, duration: Duration) {
        self.timings.push(duration);
    }

    fn p50(&self) -> Duration {
        let mut sorted = self.timings.clone();
        sorted.sort();
        sorted.get(sorted.len() / 2).copied().unwrap_or_default()
    }

    fn p99(&self) -> Duration {
        let mut sorted = self.timings.clone();
        sorted.sort();
        let idx = (sorted.len() as f64 * 0.99) as usize;
        sorted.get(idx.min(sorted.len().saturating_sub(1))).copied().unwrap_or_default()
    }

    fn mean(&self) -> Duration {
        if self.timings.is_empty() {
            return Duration::ZERO;
        }
        let total: Duration = self.timings.iter().sum();
        total / self.timings.len() as u32
    }

    fn total(&self) -> Duration {
        self.timings.iter().sum()
    }
}

struct GeneratedQueries {
    balance: Vec<BenchmarkQuery>,
    nonce: Vec<BenchmarkQuery>,
    storage: Vec<BenchmarkQuery>,
}

fn main() -> Result<()> {
    let args = Args::parse();

    match args.bench_type {
        BenchType::Changeset => run_changeset_benchmark(&args),
        BenchType::Txhash => run_txhash_benchmark(&args),
    }
}

fn run_changeset_benchmark(args: &Args) -> Result<()> {
    let normal = args.normal.as_ref().expect("--normal required for changeset benchmark");
    let edge = args.edge.as_ref().expect("--edge required for changeset benchmark");

    let sampled = if let Some(ref path) = args.sample_file {
        if path.exists() {
            println!("Loading sampled data from {:?}", path);
            let file = File::open(path)?;
            serde_json::from_reader(BufReader::new(file))?
        } else {
            let data = sample_changesets(args, normal)?;
            let file = File::create(path)?;
            serde_json::to_writer_pretty(BufWriter::new(file), &data)?;
            println!("Saved sampled data to {:?}", path);
            data
        }
    } else {
        sample_changesets(args, normal)?
    };

    println!("\n=== Sampled Data Summary ===");
    println!("Accounts with changesets: {}", sampled.accounts.len());
    println!("Storage slots with changesets: {}", sampled.storage.len());
    println!("Block range: {} - {}", sampled.oldest_block, sampled.latest_block);

    let queries = generate_queries(args, &sampled);
    println!("\nGenerated {} queries per method", queries.balance.len());

    let modes_to_run: Vec<BenchMode> = match args.mode {
        BenchMode::All => vec![BenchMode::AllCold, BenchMode::JarCached, BenchMode::AllWarm],
        mode => vec![mode],
    };

    let mut all_results: Vec<(BenchMode, ModeResults)> = Vec::new();

    for mode in modes_to_run {
        println!("\n{}", "=".repeat(60));
        println!("=== Mode: {:?} ===", mode);
        print_mode_description(mode);

        let (normal_balance, _) = run_benchmark(normal, &queries.balance, "MDBX", mode)?;
        let (edge_balance, _) = run_benchmark(edge, &queries.balance, "RocksDB/SF", mode)?;

        let (normal_nonce, _) = run_benchmark(normal, &queries.nonce, "MDBX", mode)?;
        let (edge_nonce, _) = run_benchmark(edge, &queries.nonce, "RocksDB/SF", mode)?;

        let (normal_storage, _) = run_benchmark(normal, &queries.storage, "MDBX", mode)?;
        let (edge_storage, _) = run_benchmark(edge, &queries.storage, "RocksDB/SF", mode)?;

        all_results.push((
            mode,
            ModeResults {
                balance: (normal_balance, edge_balance),
                nonce: (normal_nonce, edge_nonce),
                storage: (normal_storage, edge_storage),
            },
        ));
    }

    println!("\n{}", "=".repeat(60));
    println!("=== FINAL RESULTS ===");

    for (mode, results) in &all_results {
        println!("\n--- {:?} ---", mode);
        print_comparison("eth_getBalance", &results.balance.0, &results.balance.1);
        print_comparison("eth_getTransactionCount", &results.nonce.0, &results.nonce.1);
        print_comparison("eth_getStorageAt", &results.storage.0, &results.storage.1);
    }

    if all_results.len() > 1 {
        print_cross_mode_comparison(&all_results);
    }

    Ok(())
}

fn run_txhash_benchmark(args: &Args) -> Result<()> {
    let datadir = args.datadir.as_ref().expect("--datadir required for txhash benchmark");

    let sampled = if let Some(ref path) = args.sample_file {
        if path.exists() {
            println!("Loading sampled data from {:?}", path);
            let file = File::open(path)?;
            serde_json::from_reader(BufReader::new(file))?
        } else {
            let data = sample_tx_hashes(args, datadir)?;
            let file = File::create(path)?;
            serde_json::to_writer_pretty(BufWriter::new(file), &data)?;
            println!("Saved sampled data to {:?}", path);
            data
        }
    } else {
        sample_tx_hashes(args, datadir)?
    };

    println!("\n=== Sampled Data Summary ===");
    println!("Transaction hashes: {}", sampled.tx_hashes.len());
    println!("Block range: {} - {}", sampled.oldest_block, sampled.latest_block);

    let queries = generate_txhash_queries(args, &sampled);
    println!("\nGenerated {} TxHash queries", queries.len());

    let modes_to_run: Vec<BenchMode> = match args.mode {
        BenchMode::All => vec![BenchMode::AllCold, BenchMode::JarCached, BenchMode::AllWarm],
        mode => vec![mode],
    };

    let mut all_results: Vec<(BenchMode, TxHashModeResults)> = Vec::new();

    for mode in modes_to_run {
        println!("\n{}", "=".repeat(60));
        println!("=== Mode: {:?} ===", mode);
        print_mode_description(mode);

        let mdbx_results = run_txhash_benchmark_backend(datadir, &queries, "MDBX", mode, false)?;
        let rocksdb_results =
            run_txhash_benchmark_backend(datadir, &queries, "RocksDB", mode, true)?;

        all_results.push((mode, TxHashModeResults { mdbx: mdbx_results, rocksdb: rocksdb_results }));
    }

    println!("\n{}", "=".repeat(60));
    println!("=== FINAL RESULTS (TxHashNumbers) ===");

    for (mode, results) in &all_results {
        println!("\n--- {:?} ---", mode);
        print_comparison("TxHash -> TxNumber", &results.mdbx, &results.rocksdb);
    }

    if all_results.len() > 1 {
        print_txhash_cross_mode_comparison(&all_results);
    }

    Ok(())
}

fn sample_tx_hashes(args: &Args, datadir: &PathBuf) -> Result<SampledData> {
    println!("Opening {:?} to sample transaction hashes...", datadir);

    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

    let provider = factory.provider()?;

    let latest_block = provider.last_block_number()?;
    let oldest_block = 1u64;

    println!("Block range: {} - {} ({} blocks)", oldest_block, latest_block, latest_block);

    println!("Sampling transaction hashes across full chain...");
    let mut tx_hashes = Vec::with_capacity(args.samples);
    let mut rng = rand::thread_rng();

    let total_txs: Option<u64> =
        provider.block_body_indices(latest_block)?.map(|i| i.first_tx_num + i.tx_count);

    if let Some(max_tx) = total_txs {
        println!("  Total transactions: ~{}", max_tx);

        let step: u64 = max_tx / args.samples as u64;
        for i in 0..args.samples {
            let tx_num = i as u64 * step + rng.gen_range(0..step.max(1));
            if tx_num >= max_tx {
                continue;
            }

            if let Some(tx) = provider.transaction_by_id(tx_num)? {
                let hash = *tx.tx_hash();
                tx_hashes.push(TxHashSample { hash, tx_number: tx_num });
            }

            if (i + 1) % 100 == 0 {
                print!("\r  Sampled {} transaction hashes", tx_hashes.len());
                std::io::stdout().flush()?;
            }
        }
        println!("\r  Sampled {} transaction hashes", tx_hashes.len());
    }

    drop(provider);
    println!("Database closed.");

    Ok(SampledData {
        accounts: Vec::new(),
        storage: Vec::new(),
        tx_hashes,
        oldest_block,
        latest_block,
    })
}

fn generate_txhash_queries(args: &Args, sampled: &SampledData) -> Vec<TxHashQuery> {
    let mut rng = rand::thread_rng();
    let mut queries = Vec::with_capacity(args.queries);

    // Shuffle samples and take first N to ensure unique queries
    let mut shuffled: Vec<_> = sampled.tx_hashes.iter().collect();
    shuffled.shuffle(&mut rng);

    for sample in shuffled.into_iter().take(args.queries) {
        queries.push(TxHashQuery {
            hash: sample.hash,
            expected_tx_number: sample.tx_number,
        });
    }

    queries
}

fn run_txhash_benchmark_backend(
    datadir: &PathBuf,
    queries: &[TxHashQuery],
    backend_name: &str,
    mode: BenchMode,
    use_rocksdb: bool,
) -> Result<BenchmarkResults> {
    println!(
        "\nRunning TxHash lookup on {} ({} queries, mode={:?})...",
        backend_name,
        queries.len(),
        mode
    );

    let spec = ChainSpecBuilder::mainnet().build();
    let mut results = BenchmarkResults::new();
    let benchmark_start = Instant::now();

    match mode {
        BenchMode::AllCold => {
            for (i, query) in queries.iter().enumerate() {
                clear_os_cache_silent()?;

                let t1 = Instant::now();
                let factory = EthereumNode::provider_factory_builder()
                    .open_read_only(spec.clone().into(), ReadOnlyConfig::from_datadir(datadir))?;

                let mut settings = factory.cached_storage_settings();
                settings.transaction_hash_numbers_in_rocksdb = use_rocksdb;
                factory.set_storage_settings_cache(settings);

                let db_open_time = t1.elapsed();

                let start = Instant::now();
                let db_provider = factory.provider()?;
                let _tx_num = db_provider.transaction_id(query.hash)?;
                let query_time = start.elapsed();

                results.add(query_time);

                if i < 3 {
                    println!("\n  Query {}: db_open={:?}, query={:?}", i, db_open_time, query_time);
                }

                drop(db_provider);
                drop(factory);

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::JarCached => {
            clear_os_cache()?;

            let factory = EthereumNode::provider_factory_builder()
                .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

            let mut settings = factory.cached_storage_settings();
            settings.transaction_hash_numbers_in_rocksdb = use_rocksdb;
            factory.set_storage_settings_cache(settings);

            println!("  Warming up with first query...");
            {
                let db_provider = factory.provider()?;
                let _ = db_provider.transaction_id(queries[0].hash)?;
            }

            for (i, query) in queries.iter().enumerate() {
                clear_os_cache_silent()?;

                let start = Instant::now();
                let db_provider = factory.provider()?;
                let _tx_num = db_provider.transaction_id(query.hash)?;
                results.add(start.elapsed());

                if i < 3 {
                    println!("\n  Query {}: {:?}", i, results.timings.last().unwrap());
                }

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::AllWarm => {
            let factory = EthereumNode::provider_factory_builder()
                .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

            let mut settings = factory.cached_storage_settings();
            settings.transaction_hash_numbers_in_rocksdb = use_rocksdb;
            factory.set_storage_settings_cache(settings);

            println!("  No cache clearing, no warmup - measuring first access with DB open...");
            for (i, query) in queries.iter().enumerate() {
                let start = Instant::now();
                let db_provider = factory.provider()?;
                let _tx_num = db_provider.transaction_id(query.hash)?;
                results.add(start.elapsed());

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::All => unreachable!(),
    }
    println!();

    Ok(results)
}

fn sample_changesets(args: &Args, datadir: &PathBuf) -> Result<SampledData> {
    println!("Opening {:?} to sample changesets...", datadir);

    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

    let provider = factory.provider()?;

    let latest_block = provider.last_block_number()?;
    let oldest_block = latest_block.saturating_sub(10000);

    println!(
        "Block range: {} - {} ({} blocks)",
        oldest_block,
        latest_block,
        latest_block - oldest_block + 1
    );

    let mut accounts = Vec::new();
    let mut storage = Vec::new();

    println!("Scanning AccountChangeSets across full range...");
    {
        let mut cursor = provider.tx_ref().cursor_read::<tables::AccountChangeSets>()?;
        let mut walker = cursor.walk_range(oldest_block..=latest_block)?;

        let mut account_ranges: HashMap<Address, (BlockNumber, BlockNumber)> = HashMap::new();
        let mut count = 0;
        while let Some(Ok((block, entry))) = walker.next() {
            account_ranges
                .entry(entry.address)
                .and_modify(|(first, last)| {
                    *first = (*first).min(block);
                    *last = (*last).max(block);
                })
                .or_insert((block, block));
            count += 1;
            if count % 500000 == 0 {
                print!(
                    "\r  Scanned {} entries, found {} unique addresses",
                    count,
                    account_ranges.len()
                );
                std::io::stdout().flush()?;
            }
        }
        println!("\r  Scanned {} entries, found {} unique addresses", count, account_ranges.len());

        let mut all_entries: Vec<AccountSample> = account_ranges
            .into_iter()
            .map(|(address, (first_change, last_change))| AccountSample {
                address,
                first_change,
                last_change,
            })
            .collect();
        all_entries.sort_by_key(|s| s.first_change);

        if all_entries.len() <= args.samples {
            accounts = all_entries;
        } else {
            let step = all_entries.len() / args.samples;
            for i in 0..args.samples {
                accounts.push(all_entries[i * step].clone());
            }
        }
        println!("  Sampled {} addresses across block range", accounts.len());
    }

    println!("Scanning StorageChangeSets across full range...");
    {
        let mut storage_ranges: HashMap<(Address, B256), (BlockNumber, BlockNumber)> =
            HashMap::new();

        for block in oldest_block..=latest_block {
            if let Ok(changesets) = provider.storage_changeset(block) {
                for (bna, entry) in changesets {
                    let key = (bna.address(), entry.key);
                    storage_ranges
                        .entry(key)
                        .and_modify(|(first, last)| {
                            *first = (*first).min(block);
                            *last = (*last).max(block);
                        })
                        .or_insert((block, block));
                }
            }
            if (block - oldest_block) % 1000 == 0 {
                print!(
                    "\r  Scanned {} blocks, found {} unique slots",
                    block - oldest_block,
                    storage_ranges.len()
                );
                std::io::stdout().flush()?;
            }
        }
        println!(
            "\r  Scanned {} blocks, found {} unique slots",
            latest_block - oldest_block + 1,
            storage_ranges.len()
        );

        let mut all_entries: Vec<StorageSample> = storage_ranges
            .into_iter()
            .map(|((address, key), (first_change, last_change))| StorageSample {
                address,
                key,
                first_change,
                last_change,
            })
            .collect();
        all_entries.sort_by_key(|s| s.first_change);

        if all_entries.len() <= args.samples {
            storage = all_entries;
        } else {
            let step = all_entries.len() / args.samples;
            for i in 0..args.samples {
                storage.push(all_entries[i * step].clone());
            }
        }
        println!("  Sampled {} storage slots across block range", storage.len());
    }

    drop(provider);
    println!("Database closed.");

    Ok(SampledData {
        accounts,
        storage,
        tx_hashes: Vec::new(),
        oldest_block,
        latest_block,
    })
}

fn generate_queries(args: &Args, sampled: &SampledData) -> GeneratedQueries {
    let mut rng = rand::thread_rng();

    let mut balance = Vec::with_capacity(args.queries);
    let mut nonce = Vec::with_capacity(args.queries);
    let mut storage = Vec::with_capacity(args.queries);

    let valid_accounts: Vec<_> =
        sampled.accounts.iter().filter(|s| s.first_change > sampled.oldest_block).collect();
    let valid_storage: Vec<_> =
        sampled.storage.iter().filter(|s| s.first_change > sampled.oldest_block).collect();

    for _ in 0..args.queries {
        let sample = if !valid_accounts.is_empty() {
            valid_accounts.choose(&mut rng).copied()
        } else {
            sampled.accounts.choose(&mut rng)
        };

        if let Some(sample) = sample {
            let block = if sample.first_change > sampled.oldest_block {
                if sample.first_change < sample.last_change && rng.gen_bool(0.5) {
                    rng.gen_range(sample.first_change..sample.last_change)
                } else {
                    sample.first_change - 1
                }
            } else {
                sample.first_change
            };

            balance.push(BenchmarkQuery {
                query_type: QueryType::Balance,
                address: sample.address,
                storage_key: None,
                block,
            });
            nonce.push(BenchmarkQuery {
                query_type: QueryType::Nonce,
                address: sample.address,
                storage_key: None,
                block,
            });
        }
    }

    for _ in 0..args.queries {
        let sample = if !valid_storage.is_empty() {
            valid_storage.choose(&mut rng).copied()
        } else {
            sampled.storage.choose(&mut rng)
        };

        if let Some(sample) = sample {
            let block = if sample.first_change > sampled.oldest_block {
                if sample.first_change < sample.last_change && rng.gen_bool(0.5) {
                    rng.gen_range(sample.first_change..sample.last_change)
                } else {
                    sample.first_change - 1
                }
            } else {
                sample.first_change
            };

            storage.push(BenchmarkQuery {
                query_type: QueryType::Storage,
                address: sample.address,
                storage_key: Some(sample.key),
                block,
            });
        }
    }

    GeneratedQueries { balance, nonce, storage }
}

fn clear_os_cache() -> Result<()> {
    println!("Clearing OS page cache...");
    let status = Command::new("sudo")
        .args(["sh", "-c", "sync && echo 3 > /proc/sys/vm/drop_caches"])
        .status()?;

    if !status.success() {
        eprintln!("Warning: Failed to clear OS cache (requires sudo)");
    }
    Ok(())
}

fn clear_os_cache_silent() -> Result<()> {
    Command::new("sudo")
        .args(["sh", "-c", "sync && echo 3 > /proc/sys/vm/drop_caches"])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    Ok(())
}

fn run_benchmark(
    datadir: &PathBuf,
    queries: &[BenchmarkQuery],
    backend_name: &str,
    mode: BenchMode,
) -> Result<(BenchmarkResults, HistoryStats)> {
    let method_name = queries
        .first()
        .map(|q| match q.query_type {
            QueryType::Balance => "eth_getBalance",
            QueryType::Nonce => "eth_getTransactionCount",
            QueryType::Storage => "eth_getStorageAt",
        })
        .unwrap_or("unknown");

    println!(
        "\nRunning {} on {} ({} queries, mode={:?})...",
        method_name,
        backend_name,
        queries.len(),
        mode
    );

    let spec = ChainSpecBuilder::mainnet().build();
    let mut results = BenchmarkResults::new();
    let mut history_stats = HistoryStats::default();
    let benchmark_start = Instant::now();

    match mode {
        BenchMode::AllCold => {
            for (i, query) in queries.iter().enumerate() {
                clear_os_cache_silent()?;

                let t1 = Instant::now();
                let factory = EthereumNode::provider_factory_builder()
                    .open_read_only(spec.clone().into(), ReadOnlyConfig::from_datadir(datadir))?;
                let db_open_time = t1.elapsed();

                let start = Instant::now();
                let db_provider = factory.provider()?;
                let historical_provider =
                    HistoricalStateProviderRef::new(&db_provider, query.block + 1);

                match query.query_type {
                    QueryType::Balance | QueryType::Nonce => {
                        let history_info =
                            historical_provider.account_history_lookup(query.address)?;
                        history_stats.record(&history_info);
                        let _account = historical_provider.basic_account(&query.address)?;
                    }
                    QueryType::Storage => {
                        let key = query.storage_key.unwrap_or(B256::ZERO);
                        let history_info =
                            historical_provider.storage_history_lookup(query.address, key)?;
                        history_stats.record(&history_info);
                        let _value = historical_provider.storage(query.address, key)?;
                    }
                }

                let query_time = start.elapsed();
                results.add(query_time);

                if i < 3 {
                    println!("\n  Query {}: db_open={:?}, query={:?}", i, db_open_time, query_time);
                }

                drop(db_provider);
                drop(factory);

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::JarCached => {
            clear_os_cache()?;

            let factory = EthereumNode::provider_factory_builder()
                .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

            println!("  Warming up jar cache with first query...");
            {
                let q = &queries[0];
                let db_provider = factory.provider()?;
                let historical = HistoricalStateProviderRef::new(&db_provider, q.block + 1);
                let _ = historical.basic_account(&q.address)?;
            }

            for (i, query) in queries.iter().enumerate() {
                clear_os_cache_silent()?;

                let start = Instant::now();
                let db_provider = factory.provider()?;
                let historical_provider =
                    HistoricalStateProviderRef::new(&db_provider, query.block + 1);

                match query.query_type {
                    QueryType::Balance | QueryType::Nonce => {
                        let history_info =
                            historical_provider.account_history_lookup(query.address)?;
                        history_stats.record(&history_info);
                        let _account = historical_provider.basic_account(&query.address)?;
                    }
                    QueryType::Storage => {
                        let key = query.storage_key.unwrap_or(B256::ZERO);
                        let history_info =
                            historical_provider.storage_history_lookup(query.address, key)?;
                        history_stats.record(&history_info);
                        let _value = historical_provider.storage(query.address, key)?;
                    }
                }

                results.add(start.elapsed());

                if i < 3 {
                    println!("\n  Query {}: {:?}", i, results.timings.last().unwrap());
                }

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::AllWarm => {
            let factory = EthereumNode::provider_factory_builder()
                .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

            println!("  No cache clearing, no warmup - measuring first access with DB open...");
            for (i, query) in queries.iter().enumerate() {
                let start = Instant::now();

                let db_provider = factory.provider()?;
                let historical_provider =
                    HistoricalStateProviderRef::new(&db_provider, query.block + 1);

                match query.query_type {
                    QueryType::Balance | QueryType::Nonce => {
                        let history_info =
                            historical_provider.account_history_lookup(query.address)?;
                        history_stats.record(&history_info);
                        let _account = historical_provider.basic_account(&query.address)?;
                    }
                    QueryType::Storage => {
                        let key = query.storage_key.unwrap_or(B256::ZERO);
                        let history_info =
                            historical_provider.storage_history_lookup(query.address, key)?;
                        history_stats.record(&history_info);
                        let _value = historical_provider.storage(query.address, key)?;
                    }
                }

                results.add(start.elapsed());

                print_progress(i, queries.len(), &results, benchmark_start)?;
            }
        }
        BenchMode::All => unreachable!(),
    }
    println!();

    history_stats.print(backend_name);

    Ok((results, history_stats))
}

fn print_progress(
    i: usize,
    total: usize,
    results: &BenchmarkResults,
    benchmark_start: Instant,
) -> Result<()> {
    if (i + 1) % 10 == 0 || i + 1 == total {
        let completed = i + 1;
        let remaining = total - completed;
        let elapsed = benchmark_start.elapsed();
        let avg_per_query = elapsed / completed as u32;
        let eta = avg_per_query * remaining as u32;

        print!(
            "\r  Progress: {}/{} | Avg: {:.1?} | ETA: {:.1?}    ",
            completed,
            total,
            results.mean(),
            eta
        );
        std::io::stdout().flush()?;
    }
    Ok(())
}

fn print_mode_description(mode: BenchMode) {
    match mode {
        BenchMode::AllCold => {
            println!("  Clear OS cache + reopen DB before each query");
            println!("  Measures: jar loading + page faults + query");
        }
        BenchMode::JarCached => {
            println!("  Keep DB open, clear OS cache before each query");
            println!("  Measures: page faults + query (jar already loaded)");
        }
        BenchMode::AllWarm => {
            println!("  Keep DB open, no cache clearing");
            println!("  Measures: first access performance with warm DB");
        }
        BenchMode::All => unreachable!(),
    }
}

fn print_comparison(method: &str, mdbx: &BenchmarkResults, rocksdb: &BenchmarkResults) {
    println!("\n{}", method);
    println!("  {:12} {:>10} {:>10} {:>10} {:>12}", "Backend", "P50", "P99", "Mean", "Total");
    println!(
        "  {:12} {:>10.2?} {:>10.2?} {:>10.2?} {:>12.2?}",
        "MDBX",
        mdbx.p50(),
        mdbx.p99(),
        mdbx.mean(),
        mdbx.total()
    );
    println!(
        "  {:12} {:>10.2?} {:>10.2?} {:>10.2?} {:>12.2?}",
        "RocksDB",
        rocksdb.p50(),
        rocksdb.p99(),
        rocksdb.mean(),
        rocksdb.total()
    );

    let diff_pct = if mdbx.p50() > Duration::ZERO {
        let mdbx_ms = mdbx.p50().as_secs_f64() * 1000.0;
        let rocks_ms = rocksdb.p50().as_secs_f64() * 1000.0;
        ((rocks_ms - mdbx_ms) / mdbx_ms) * 100.0
    } else {
        0.0
    };

    if diff_pct.abs() > 1.0 {
        println!(
            "  Difference (P50): {:+.1}% ({})",
            diff_pct,
            if diff_pct > 0.0 { "RocksDB slower" } else { "MDBX slower" }
        );
    } else {
        println!("  Difference (P50): ~0% (within noise)");
    }
}

fn print_cross_mode_comparison(results: &[(BenchMode, ModeResults)]) {
    println!("\n=== Cross-Mode Summary (P50 latencies) ===");

    let methods: &[(&str, fn(&ModeResults) -> (&BenchmarkResults, &BenchmarkResults))] = &[
        ("eth_getBalance", |r| (&r.balance.0, &r.balance.1)),
        ("eth_getTransactionCount", |r| (&r.nonce.0, &r.nonce.1)),
        ("eth_getStorageAt", |r| (&r.storage.0, &r.storage.1)),
    ];

    for (method_name, get_results) in methods {
        println!("\n{}", method_name);
        println!(
            "  {:15} {:>12} {:>12} {:>12}",
            "Mode", "MDBX P50", "RocksDB P50", "Ratio"
        );

        for (mode, mode_results) in results {
            let (mdbx, rocksdb) = get_results(mode_results);
            let ratio = if mdbx.p50() > Duration::ZERO {
                rocksdb.p50().as_secs_f64() / mdbx.p50().as_secs_f64()
            } else {
                0.0
            };
            println!(
                "  {:15} {:>12.2?} {:>12.2?} {:>11.1}x",
                format!("{:?}", mode),
                mdbx.p50(),
                rocksdb.p50(),
                ratio
            );
        }
    }

    println!("\n=== Interpretation ===");
    println!("  AllCold:   Full cold start (jar load + page faults + query)");
    println!("  JarCached: Page fault cost with jar already loaded");
    println!("  AllWarm:   First access performance with warm DB");
}

fn print_txhash_cross_mode_comparison(results: &[(BenchMode, TxHashModeResults)]) {
    println!("\n=== Cross-Mode Summary (TxHashNumbers P50 latencies) ===");

    println!(
        "\n  {:15} {:>12} {:>12} {:>12}",
        "Mode", "MDBX P50", "RocksDB P50", "Ratio"
    );

    for (mode, mode_results) in results {
        let ratio = if mode_results.mdbx.p50() > Duration::ZERO {
            mode_results.rocksdb.p50().as_secs_f64() / mode_results.mdbx.p50().as_secs_f64()
        } else {
            0.0
        };
        println!(
            "  {:15} {:>12.2?} {:>12.2?} {:>11.1}x",
            format!("{:?}", mode),
            mode_results.mdbx.p50(),
            mode_results.rocksdb.p50(),
            ratio
        );
    }

    println!("\n=== Interpretation ===");
    println!("  AllCold:   Full cold start (DB open + page faults + query)");
    println!("  JarCached: Page fault cost with DB already open");
    println!("  AllWarm:   First access performance with warm DB");
}
