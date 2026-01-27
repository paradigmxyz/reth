//! Benchmark tool for comparing MDBX vs RocksDB/StaticFiles historical query performance.
//!
//! This tool:
//! 1. Opens the first datadir to collect sample addresses/blocks with actual changesets
//! 2. Closes the database
//! 3. Clears OS page cache before each query (requires sudo)
//! 4. Runs benchmarks against both datadirs measuring true cold read performance

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
use serde::{Deserialize, Serialize};
use std::{
    collections::HashSet,
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
    #[arg(long, default_value = "200")]
    queries: usize,

    /// Number of addresses/storage slots to sample
    #[arg(long, default_value = "50")]
    samples: usize,

    /// Percentage of queries targeting oldest blocks (worst case)
    #[arg(long, default_value = "70")]
    oldest_pct: u8,

    /// Skip cache clearing (for testing without sudo)
    #[arg(long)]
    skip_cache_clear: bool,

    /// Path to save/load sampled data (skip sampling if exists)
    #[arg(long)]
    sample_file: Option<PathBuf>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct SampledData {
    /// Addresses with account changesets
    accounts: Vec<(Address, BlockNumber)>,
    /// (Address, StorageKey, BlockNumber) tuples with storage changesets
    storage: Vec<(Address, B256, BlockNumber)>,
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
    println!("  ~{}% targeting oldest 10% of blocks (worst case)", args.oldest_pct);

    println!("\n=== Running Benchmarks ===");

    if !args.skip_cache_clear {
        clear_os_cache()?;
    }

    let clear_per_query = !args.skip_cache_clear;

    let normal_balance = run_benchmark(&args.normal, &queries.balance, "MDBX", clear_per_query)?;
    let edge_balance = run_benchmark(&args.edge, &queries.balance, "RocksDB/SF", clear_per_query)?;

    let normal_nonce = run_benchmark(&args.normal, &queries.nonce, "MDBX", clear_per_query)?;
    let edge_nonce = run_benchmark(&args.edge, &queries.nonce, "RocksDB/SF", clear_per_query)?;

    let normal_storage = run_benchmark(&args.normal, &queries.storage, "MDBX", clear_per_query)?;
    let edge_storage = run_benchmark(&args.edge, &queries.storage, "RocksDB/SF", clear_per_query)?;

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

    let mut accounts: Vec<(Address, BlockNumber)> = Vec::new();
    let mut storage: Vec<(Address, B256, BlockNumber)> = Vec::new();
    let mut seen_addresses: HashSet<Address> = HashSet::new();
    let mut seen_storage: HashSet<(Address, B256)> = HashSet::new();

    println!("Scanning AccountChangeSets across full range...");
    {
        let mut cursor = provider.tx_ref().cursor_read::<tables::AccountChangeSets>()?;
        let mut walker = cursor.walk_range(oldest_block..=latest_block)?;

        let mut all_entries: Vec<(Address, BlockNumber)> = Vec::new();
        let mut count = 0;
        while let Some(Ok((block, entry))) = walker.next() {
            if !seen_addresses.contains(&entry.address) {
                seen_addresses.insert(entry.address);
                all_entries.push((entry.address, block));
            }
            count += 1;
            if count % 500000 == 0 {
                print!(
                    "\r  Scanned {} entries, found {} unique addresses",
                    count,
                    all_entries.len()
                );
                std::io::stdout().flush()?;
            }
        }
        println!(
            "\r  Scanned {} entries, found {} unique addresses",
            count,
            all_entries.len()
        );

        // Sample evenly across the collected entries
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
        let mut all_storage: Vec<(Address, B256, BlockNumber)> = Vec::new();

        for block in oldest_block..=latest_block {
            if let Ok(changesets) = provider.storage_changeset(block) {
                for (bna, entry) in changesets {
                    let key = (bna.address(), entry.key);
                    if !seen_storage.contains(&key) {
                        seen_storage.insert(key);
                        all_storage.push((bna.address(), entry.key, bna.block_number()));
                    }
                }
            }
            if (block - oldest_block) % 1000 == 0 {
                print!(
                    "\r  Scanned {} blocks, found {} unique slots",
                    block - oldest_block,
                    all_storage.len()
                );
                std::io::stdout().flush()?;
            }
        }
        println!(
            "\r  Scanned {} blocks, found {} unique slots",
            latest_block - oldest_block + 1,
            all_storage.len()
        );

        // Sample evenly across the collected entries
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

    let history_depth = sampled.latest_block - sampled.oldest_block;
    let oldest_10pct = sampled.oldest_block + history_depth / 10;

    let gen_block = |rng: &mut rand::rngs::ThreadRng| -> BlockNumber {
        if rng.gen_range(0..100) < args.oldest_pct {
            rng.gen_range(sampled.oldest_block..=oldest_10pct)
        } else {
            rng.gen_range(sampled.oldest_block..=sampled.latest_block)
        }
    };

    let mut balance = Vec::with_capacity(args.queries);
    let mut nonce = Vec::with_capacity(args.queries);
    let mut storage = Vec::with_capacity(args.queries);

    for _ in 0..args.queries {
        if let Some((address, _)) = sampled.accounts.choose(&mut rng) {
            balance.push(BenchmarkQuery {
                query_type: QueryType::Balance,
                address: *address,
                storage_key: None,
                block: gen_block(&mut rng),
            });
            nonce.push(BenchmarkQuery {
                query_type: QueryType::Nonce,
                address: *address,
                storage_key: None,
                block: gen_block(&mut rng),
            });
        }
    }

    for _ in 0..args.queries {
        if let Some((address, key, _)) = sampled.storage.choose(&mut rng) {
            storage.push(BenchmarkQuery {
                query_type: QueryType::Storage,
                address: *address,
                storage_key: Some(*key),
                block: gen_block(&mut rng),
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
    clear_cache_per_query: bool,
) -> Result<BenchmarkResults> {
    let method_name = queries
        .first()
        .map(|q| match q.query_type {
            QueryType::Balance => "eth_getBalance",
            QueryType::Nonce => "eth_getTransactionCount",
            QueryType::Storage => "eth_getStorageAt",
        })
        .unwrap_or("unknown");

    println!(
        "\nRunning {} on {} ({} queries, cache_clear_per_query={})...",
        method_name,
        backend_name,
        queries.len(),
        clear_cache_per_query
    );

    let spec = ChainSpecBuilder::mainnet().build();
    let factory = EthereumNode::provider_factory_builder()
        .open_read_only(spec.into(), ReadOnlyConfig::from_datadir(datadir))?;

    let mut results = BenchmarkResults::new();

    for (i, query) in queries.iter().enumerate() {
        if clear_cache_per_query {
            clear_os_cache_silent()?;
        }

        let start = Instant::now();

        let state = factory.history_by_block_number(query.block)?;

        match query.query_type {
            QueryType::Balance | QueryType::Nonce => {
                let _account = state.basic_account(&query.address)?;
            }
            QueryType::Storage => {
                let key = query.storage_key.unwrap_or(B256::ZERO);
                let _value = state.storage(query.address, key)?;
            }
        }

        results.add(start.elapsed());

        if (i + 1) % 10 == 0 || i + 1 == queries.len() {
            print!("\r  Progress: {}/{}", i + 1, queries.len());
            std::io::stdout().flush()?;
        }
    }
    println!("\r  Completed {} queries", queries.len());

    Ok(results)
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
