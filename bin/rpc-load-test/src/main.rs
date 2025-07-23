//! RPC load testing tool that continuously traces random recent blocks.
//!
//! Usage:
//! ```bash
//! cargo run --bin rpc-load-test -- --rpc-url http://localhost:8545 --concurrency 20 --block-window 100
//! ```

use alloy_rpc_types_eth::{Block, BlockId, Header, Transaction, TransactionRequest};
use clap::Parser;
use jsonrpsee::http_client::{HttpClient, HttpClientBuilder};
use reth_ethereum_primitives::Receipt;
use reth_rpc_api::clients::TraceApiClient;
use reth_rpc_eth_api::EthApiClient;
use std::{
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
    time::Instant,
};
use tokio::{
    sync::RwLock,
    time::{interval, Duration},
};

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// RPC endpoint URL
    #[arg(short, long, default_value = "http://localhost:8545")]
    rpc_url: String,

    /// Number of concurrent workers
    #[arg(short, long, default_value_t = 10)]
    concurrency: usize,

    /// Number of recent blocks to randomly select from
    #[arg(short, long, default_value_t = 50)]
    block_window: u64,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let args = Args::parse();

    let url = args.rpc_url.clone();
    println!("🚀 Starting load test against: {}", url);

    // Create HTTP client
    let client = Arc::new(HttpClientBuilder::default().build(args.rpc_url)?);

    // Configuration from CLI args
    let concurrency = args.concurrency;
    let block_window = args.block_window;

    // Shared state for the latest block number
    let latest_block = Arc::new(RwLock::new(0u64));

    // Shared counters for statistics
    let total_requests = Arc::new(AtomicU64::new(0));
    let total_errors = Arc::new(AtomicU64::new(0));
    // Clone for the block update task
    let latest_block_updater = Arc::clone(&latest_block);
    let client_for_updater = Arc::clone(&client);

    // Task: Periodically fetch and update the latest block number
    // This task runs every 5 seconds to keep track of the chain tip
    tokio::spawn(async move {
        let mut update_interval = interval(Duration::from_secs(5));
        loop {
            update_interval.tick().await;
            match fetch_latest_block(&client_for_updater).await {
                Ok(block_num) => {
                    let mut latest = latest_block_updater.write().await;
                    *latest = block_num;
                    println!("📦 Updated latest block: {}", block_num);
                }
                Err(e) => {
                    eprintln!("❌ Failed to fetch latest block: {}", e);
                }
            }
        }
    });

    // Wait for initial block number
    println!("⏳ Fetching initial block number...");
    let initial_block = fetch_latest_block(&client).await?;
    {
        let mut latest = latest_block.write().await;
        *latest = initial_block;
    }
    println!("📦 Starting with block: {}", initial_block);

    println!("📊 Continuously tracing random blocks from the last {} blocks", block_window);
    println!("🔄 Using {} concurrent workers", concurrency);
    println!("   Press Ctrl+C to stop\n");

    let start_time = Instant::now();

    // Spawn concurrent tasks for load generation
    let mut handles = vec![];

    for i in 0..concurrency {
        let client_clone = Arc::clone(&client);
        let latest_block_clone = Arc::clone(&latest_block);
        let total_requests_clone = Arc::clone(&total_requests);
        let total_errors_clone = Arc::clone(&total_errors);

        // Task: Worker that continuously traces random blocks
        // Each worker selects a random block from the recent block window and calls trace_block
        let handle = tokio::spawn(async move {
            // Add initial random delay to spread out workers
            let initial_jitter = rand::random_range(0..1000);
            tokio::time::sleep(Duration::from_millis(initial_jitter)).await;

            loop {
                // Get current latest block
                let current_latest = {
                    let latest = latest_block_clone.read().await;
                    *latest
                };

                // Skip if we don't have enough blocks yet
                if current_latest < block_window {
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    continue;
                }

                // Pick a random block to trace
                let block_num =
                    rand::random_range((current_latest - block_window + 1)..=current_latest);
                let block_id = BlockId::Number(block_num.into());

                // Trace the block
                match <HttpClient as TraceApiClient<TransactionRequest>>::trace_block(
                    &client_clone,
                    block_id,
                )
                .await
                {
                    Ok(Some(traces)) => {
                        let count = total_requests_clone.fetch_add(1, Ordering::Relaxed);
                        if (count + 1) % 100 == 0 {
                            println!(
                                "✅ Worker {}: Traced block {}: {} traces",
                                i,
                                block_num,
                                traces.len()
                            );
                        }
                    }
                    Ok(None) => {
                        total_requests_clone.fetch_add(1, Ordering::Relaxed);
                        // Block doesn't exist or has no traces
                    }
                    Err(err) => {
                        total_requests_clone.fetch_add(1, Ordering::Relaxed);
                        let error_count = total_errors_clone.fetch_add(1, Ordering::Relaxed);
                        if (error_count + 1) % 10 == 0 {
                            eprintln!(
                                "❌ Worker {}: Error tracing block {}: {}",
                                i, block_num, err
                            );
                        }
                    }
                }

                // Add jitter between requests (0-100ms)
                let jitter = rand::random_range(0..100);
                tokio::time::sleep(Duration::from_millis(jitter)).await;
            }
        });

        handles.push(handle);
    }

    // Spawn stats reporting task
    let total_requests_stats = Arc::clone(&total_requests);
    let total_errors_stats = Arc::clone(&total_errors);
    let latest_block_stats = Arc::clone(&latest_block);

    // Task: Report statistics every 5 seconds
    // Shows RPS, total requests, errors, and current latest block
    tokio::spawn(async move {
        let mut report_interval = interval(Duration::from_secs(5));
        let mut last_report_requests = 0u64;
        let mut last_report_time = Instant::now();

        loop {
            report_interval.tick().await;

            let now = Instant::now();
            let current_requests = total_requests_stats.load(Ordering::Relaxed);
            let current_errors = total_errors_stats.load(Ordering::Relaxed);
            let current_latest = {
                let latest = latest_block_stats.read().await;
                *latest
            };

            let period_duration = now.duration_since(last_report_time);
            let period_requests = current_requests - last_report_requests;
            let rps = period_requests as f64 / period_duration.as_secs_f64();

            let total_elapsed = now.duration_since(start_time);
            let avg_rps = current_requests as f64 / total_elapsed.as_secs_f64();
            let error_rate = if current_requests > 0 {
                (current_errors as f64 / current_requests as f64) * 100.0
            } else {
                0.0
            };

            println!("⏱️  [{:>6.1}s] RPS: {:>6.1} (avg: {:>6.1}) | Total: {:>8} | Errors: {:>6} ({:.1}%) | Latest Block: {}",
                total_elapsed.as_secs_f64(),
                rps,
                avg_rps,
                current_requests,
                current_errors,
                error_rate,
                current_latest
            );

            last_report_time = now;
            last_report_requests = current_requests;
        }
    });

    // Wait for all tasks (they run forever)
    for handle in handles {
        handle.await?;
    }

    Ok(())
}

async fn fetch_latest_block(
    client: &HttpClient,
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    let block_number = <HttpClient as EthApiClient<
        TransactionRequest,
        Transaction,
        Block,
        Receipt,
        Header,
    >>::block_number(client)
    .await?;

    Ok(block_number.try_into()?)
}
