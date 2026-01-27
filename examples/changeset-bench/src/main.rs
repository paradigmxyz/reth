//! Benchmark tool for comparing MDBX vs RocksDB/StaticFiles historical query performance.
//!
//! This tool:
//! 1. Opens the first datadir to collect sample addresses/blocks with actual changesets
//! 2. Closes the database
//! 3. Runs benchmarks against both datadirs
//!
//! By default (warm mode): clears OS cache once before each backend run
//! With --cold flag: clears OS cache AND reopens database before EACH query

use alloy_primitives::{Address, BlockNumber, B256};
use clap::Parser;
use eyre::Result;
use rand::{seq::SliceRandom, Rng};
use reth_db_api::{cursor::DbCursorRO, tables, transaction::DbTx};
use reth_ethereum::{
    chainspec::ChainSpecBuilder,
    node::EthereumNode,
    provider::{
        providers::ReadOnlyConfig, AccountReader, BlockNumReader, StateProvider,
        StorageChangeSetReader,
    },
};
use reth_provider::providers::{HistoricalStateProviderRef, HistoryInfo};
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
#[command(about = "Benchmark MDBX vs RocksDB/SF historical query performance")]
struct Args {
    /// Path to the normal (MDBX) datadir
    #[arg(long)]
    normal: PathBuf,

    /// Path to the edge (RocksDB/SF) datadir
    #[arg(long)]
    edge: PathBuf,

    /// Number of queries to run per method
    #[arg(long, default_value = "2000")]
    queries: usize,

    /// Number of addresses/storage slots to sample
    #[arg(long, default_value = "50")]
    samples: usize,

    /// Cold cache mode: clear OS cache AND reopen database before EACH query
    /// Without this flag (warm mode): clear OS cache once before each backend run
    #[arg(long)]
    cold: bool,

    /// Path to save/load sampled data (skip sampling if exists)
    #[arg(long)]
    sample_file: Option<PathBuf>,
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
struct SampledData {
    /// Addresses with account changesets (first and last change blocks)
    accounts: Vec<AccountSample>,
    /// Storage slots with changesets (first and last change blocks)
    storage: Vec<StorageSample>,
    /// Block range
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

fn main() -> Result<()> {
    let args = Args::parse();

    let sampled = if let Some(ref path) = args.sample_file {
        if path.exists() {
            println!("Loading sampled data from {:?}", path);
            let file = File::open(path)?;
            serde_json::from_reader(BufReader::new(file))?
        } else {
            let data = sample_changesets(&args)?;
            let file = File::create(path)?;
            serde_json::to_writer_pretty(BufWriter::new(file), &data)?;
            println!("Saved sampled data to {:?}", path);
            data
        }
    } else {
        sample_changesets(&args)?
    };

    println!("\n=== Sampled Data Summary ===");
    println!("Accounts with changesets: {}", sampled.accounts.len());
    println!("Storage slots with changesets: {}", sampled.storage.len());
    println!("Block range: {} - {}", sampled.oldest_block, sampled.latest_block);

    let queries = generate_queries(&args, &sampled);
    println!("\nGenerated {} queries per method", queries.balance.len());
    println!("  Query strategy: ~33% before first change, ~33% mid-range, ~33% at last change");

    println!("\n=== Running Benchmarks ===");
    if args.cold {
        println!("Mode: COLD (clearing OS cache + reopening DB before each query)");
    } else {
        println!("Mode: WARM (clearing OS cache once before each backend run)");
    }

    let (normal_balance, _) = run_benchmark(&args.normal, &queries.balance, "MDBX", args.cold)?;
    let (edge_balance, _) = run_benchmark(&args.edge, &queries.balance, "RocksDB/SF", args.cold)?;

    let (normal_nonce, _) = run_benchmark(&args.normal, &queries.nonce, "MDBX", args.cold)?;
    let (edge_nonce, _) = run_benchmark(&args.edge, &queries.nonce, "RocksDB/SF", args.cold)?;

    let (normal_storage, _) = run_benchmark(&args.normal, &queries.storage, "MDBX", args.cold)?;
    let (edge_storage, _) = run_benchmark(&args.edge, &queries.storage, "RocksDB/SF", args.cold)?;

    println!("\n=== Results ===");
    print_comparison("eth_getBalance", &normal_balance, &edge_balance);
    print_comparison("eth_getTransactionCount (nonce)", &normal_nonce, &edge_nonce);
    print_comparison("eth_getStorageAt", &normal_storage, &edge_storage);

    Ok(())
}

fn sample_changesets(args: &Args) -> Result<SampledData> {
    println!("Opening {:?} to sample changesets...", args.normal);

    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(&args.normal))?;

    let provider = factory.provider()?;

    let latest_block = provider.last_block_number()?;
    let oldest_block = latest_block.saturating_sub(10000);

    println!(
        "Block range: {} - {} ({} blocks)",
        oldest_block,
        latest_block,
        latest_block - oldest_block + 1
    );

    let mut accounts: Vec<AccountSample> = Vec::new();
    let mut storage: Vec<StorageSample> = Vec::new();

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

        let mut all_storage: Vec<StorageSample> = storage_ranges
            .into_iter()
            .map(|((address, key), (first_change, last_change))| StorageSample {
                address,
                key,
                first_change,
                last_change,
            })
            .collect();
        all_storage.sort_by_key(|s| s.first_change);

        if all_storage.len() <= args.samples {
            storage = all_storage;
        } else {
            let step = all_storage.len() / args.samples;
            for i in 0..args.samples {
                storage.push(all_storage[i * step].clone());
            }
        }
        println!("  Sampled {} storage slots across block range", storage.len());
    }

    drop(provider);
    drop(factory);

    println!("Database closed.");

    Ok(SampledData { accounts, storage, oldest_block, latest_block })
}

struct GeneratedQueries {
    balance: Vec<BenchmarkQuery>,
    nonce: Vec<BenchmarkQuery>,
    storage: Vec<BenchmarkQuery>,
}

fn generate_queries(args: &Args, sampled: &SampledData) -> GeneratedQueries {
    let mut rng = rand::thread_rng();

    let mut balance = Vec::with_capacity(args.queries);
    let mut nonce = Vec::with_capacity(args.queries);
    let mut storage = Vec::with_capacity(args.queries);

    for _ in 0..args.queries {
        if let Some(sample) = sampled.accounts.choose(&mut rng) {
            let query_variant = rng.gen_range(0..3);
            let block = match query_variant {
                0 => {
                    if sample.first_change > sampled.oldest_block {
                        sample.first_change - 1
                    } else {
                        sample.first_change
                    }
                }
                1 => (sample.first_change + sample.last_change) / 2,
                _ => sample.last_change,
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
        if let Some(sample) = sampled.storage.choose(&mut rng) {
            let query_variant = rng.gen_range(0..3);
            let block = match query_variant {
                0 => {
                    if sample.first_change > sampled.oldest_block {
                        sample.first_change - 1
                    } else {
                        sample.first_change
                    }
                }
                1 => (sample.first_change + sample.last_change) / 2,
                _ => sample.last_change,
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
    cold_mode: bool,
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
        "\nRunning {} on {} ({} queries, cold={})...",
        method_name,
        backend_name,
        queries.len(),
        cold_mode
    );

    let spec = ChainSpecBuilder::mainnet().build();

    if !cold_mode {
        clear_os_cache()?;
    }

    let mut results = BenchmarkResults::new();
    let mut history_stats = HistoryStats::default();
    let benchmark_start = Instant::now();

    if cold_mode {
        for (i, query) in queries.iter().enumerate() {
            clear_os_cache_silent()?;

            let start = Instant::now();

            let factory = EthereumNode::provider_factory_builder()
                .open_read_only(spec.clone().into(), ReadOnlyConfig::from_datadir(datadir))?;

            let db_provider = factory.provider()?;
            let historical_provider =
                HistoricalStateProviderRef::new(&db_provider, query.block + 1);

            match query.query_type {
                QueryType::Balance | QueryType::Nonce => {
                    let history_info = historical_provider.account_history_lookup(query.address)?;
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
            drop(db_provider);
            drop(factory);

            print_progress(i, queries.len(), &results, benchmark_start)?;
        }
    } else {
        let factory = EthereumNode::provider_factory_builder()
            .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

        for (i, query) in queries.iter().enumerate() {
            let start = Instant::now();

            let db_provider = factory.provider()?;
            let historical_provider =
                HistoricalStateProviderRef::new(&db_provider, query.block + 1);

            match query.query_type {
                QueryType::Balance | QueryType::Nonce => {
                    let history_info = historical_provider.account_history_lookup(query.address)?;
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
        "RocksDB/SF",
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
            if diff_pct > 0.0 { "RocksDB/SF slower" } else { "MDBX slower" }
        );
    } else {
        println!("  Difference (P50): ~0% (within noise)");
    }
}
