//! P2P network scraper that discovers Ethereum nodes and collects their metadata.

#![warn(unused_crate_dependencies)]
#![allow(unreachable_pub)]

mod crawler;
mod geoip;
mod metrics;
mod rechecker;
mod storage;
mod types;

use clap::{Parser, Subcommand};
use crawler::{Crawler, CrawlerConfig};
use geoip::GeoIpResolver;
use metrics_exporter_prometheus::PrometheusBuilder;
use rechecker::{RecheckConfig, Rechecker};
use reth_chainspec::ChainSpec;
use reth_ethereum::chainspec::{HOLESKY, MAINNET, SEPOLIA};
use reth_network_peers::NodeRecord;
use reth_tracing::{tracing::info, RethTracer, Tracer};
use std::{sync::Arc, time::Duration};
use storage::Storage;

#[derive(Parser)]
#[command(name = "reth-scraper")]
#[command(about = "P2P network scraper that discovers Ethereum nodes and collects their metadata")]
struct Cli {
    #[arg(long, default_value = "nodes.db")]
    db: String,

    /// Chain to crawl: mainnet, sepolia, holesky
    #[arg(long, default_value = "mainnet")]
    chain: String,

    /// Custom bootnodes (comma-separated enode URLs)
    #[arg(long, value_delimiter = ',')]
    bootnodes: Option<Vec<String>>,

    #[arg(long, default_value = "50")]
    workers: usize,

    #[arg(long, default_value = "9001")]
    metrics_port: u16,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// One-shot crawl of the network
    Crawl,
    /// Continuous crawling mode with discovery and rechecker
    Run {
        /// How long to run discovery before starting next cycle
        #[arg(long, default_value = "5m")]
        interval: humantime::Duration,

        /// How often to recheck existing nodes for liveness
        #[arg(long, default_value = "30s")]
        recheck_interval: humantime::Duration,

        /// Consider nodes stale if not checked within this duration
        #[arg(long, default_value = "1h")]
        recheck_max_age: humantime::Duration,

        /// Number of nodes to recheck per batch
        #[arg(long, default_value = "100")]
        recheck_batch_size: u32,

        /// Mark node as dead after this many consecutive failures
        #[arg(long, default_value = "3")]
        max_failures: u32,
    },
    /// Show statistics from the database
    Stats {
        #[arg(long)]
        by_version: bool,
        #[arg(long)]
        by_country: bool,
    },
    /// Export nodes to JSON or CSV
    Export {
        #[arg(long, default_value = "json")]
        format: String,
    },
}

fn parse_chainspec(chain: &str) -> eyre::Result<Arc<ChainSpec>> {
    match chain.to_lowercase().as_str() {
        "mainnet" | "1" => Ok(MAINNET.clone()),
        "sepolia" | "11155111" => Ok(SEPOLIA.clone()),
        "holesky" | "17000" => Ok(HOLESKY.clone()),
        _ => Err(eyre::eyre!(
            "Unknown chain: {}. Supported: mainnet, sepolia, holesky",
            chain
        )),
    }
}

#[tokio::main]
async fn main() -> eyre::Result<()> {
    let _ = RethTracer::new().init()?;
    let cli = Cli::parse();

    let storage = Storage::new(&cli.db).await?;
    storage.init_schema().await?;

    let chainspec = parse_chainspec(&cli.chain)?;

    let bootnodes: Vec<NodeRecord> = cli
        .bootnodes
        .unwrap_or_default()
        .iter()
        .filter_map(|s| s.parse().ok())
        .collect();

    match cli.command {
        Commands::Crawl => {
            start_metrics_server(cli.metrics_port)?;
            run_crawl(&storage, chainspec, bootnodes, cli.workers, None).await?;
        }
        Commands::Run {
            interval,
            recheck_interval,
            recheck_max_age,
            recheck_batch_size,
            max_failures,
        } => {
            start_metrics_server(cli.metrics_port)?;
            run_continuous(
                &storage,
                chainspec,
                bootnodes,
                cli.workers,
                interval.into(),
                recheck_interval.into(),
                recheck_max_age.into(),
                recheck_batch_size,
                max_failures,
            )
            .await?;
        }
        Commands::Stats { by_version, by_country } => {
            let total = storage.count_nodes().await?;
            let alive = storage.count_alive_nodes().await?;
            println!("Total nodes: {} ({} alive)", total, alive);

            if by_version {
                println!("\nBy client version (alive only):");
                for (version, cnt) in storage.get_stats_by_version().await? {
                    println!("  {}: {}", version, cnt);
                }
            }

            if by_country {
                println!("\nBy country (alive only):");
                for (country, cnt) in storage.get_stats_by_country().await? {
                    println!("  {}: {}", country, cnt);
                }
            }
        }
        Commands::Export { format } => {
            let nodes = storage.get_all_nodes().await?;
            match format.as_str() {
                "json" => {
                    println!("{}", serde_json::to_string_pretty(&nodes)?);
                }
                "csv" => {
                    println!("node_id,enode,ip,tcp_port,udp_port,client_version,capabilities,eth_version,chain_id,country_code,first_seen,last_seen,is_alive");
                    for node in nodes {
                        println!(
                            "{:?},{},{},{},{},{},{},{},{},{},{},{},{}",
                            node.node_id,
                            node.enode,
                            node.ip,
                            node.tcp_port,
                            node.udp_port,
                            node.client_version.replace(',', ";"),
                            node.capabilities_string(),
                            node.eth_version.map(|v| v.to_string()).unwrap_or_default(),
                            node.chain_id.map(|v| v.to_string()).unwrap_or_default(),
                            node.country_code.unwrap_or_default(),
                            node.first_seen,
                            node.last_seen,
                            node.is_alive
                        );
                    }
                }
                _ => {
                    return Err(eyre::eyre!("Unknown format: {}. Use 'json' or 'csv'", format));
                }
            }
        }
    }

    Ok(())
}

fn start_metrics_server(port: u16) -> eyre::Result<()> {
    metrics::describe_metrics();
    PrometheusBuilder::new().with_http_listener(([0, 0, 0, 0], port)).install()?;
    info!("Metrics server started on port {}", port);
    Ok(())
}

#[allow(clippy::too_many_arguments)]
async fn run_continuous(
    storage: &Storage,
    chainspec: Arc<ChainSpec>,
    bootnodes: Vec<NodeRecord>,
    workers: usize,
    crawl_interval: Duration,
    recheck_interval: Duration,
    recheck_max_age: Duration,
    recheck_batch_size: u32,
    max_failures: u32,
) -> eyre::Result<()> {
    let geoip = GeoIpResolver::new(None)?;
    let crawler_config = CrawlerConfig {
        chainspec: chainspec.clone(),
        bootnodes,
        workers,
    };
    let crawler = Crawler::new(crawler_config, geoip.clone());

    let recheck_config = RecheckConfig {
        chainspec,
        batch_size: recheck_batch_size,
        max_age_secs: recheck_max_age.as_secs(),
        check_interval: recheck_interval,
        max_failures,
        workers: workers / 2, // Use half the workers for rechecking
    };
    let rechecker = Rechecker::new(recheck_config, storage.clone(), geoip, crawler.secret_key());

    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    // Storage handler task
    let storage_handle = {
        let storage = storage.clone();
        tokio::spawn(async move {
            let mut count = 0u64;
            while let Some(node) = rx.recv().await {
                if let Err(e) = storage.upsert_node(&node).await {
                    tracing::error!("Failed to store node: {}", e);
                } else {
                    count += 1;
                    if count.is_multiple_of(100) && storage.count_nodes().await.is_ok_and(|total| {
                        metrics::set_unique_nodes(total as u64);
                        true
                    }) {}
                }
            }
            count
        })
    };

    // Crawler task - discovers new nodes
    let crawler_stop_rx = stop_rx.clone();
    let crawler_handle = tokio::spawn(async move { crawler.run(tx, crawler_stop_rx).await });

    // Rechecker task - verifies existing nodes are still alive
    let rechecker_stop_rx = stop_rx.clone();
    let rechecker_handle = tokio::spawn(async move { rechecker.run(rechecker_stop_rx).await });

    // Periodic status logging
    let status_storage = storage.clone();
    let status_stop_rx = stop_rx.clone();
    let status_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(crawl_interval);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    if let (Ok(total), Ok(alive)) = (
                        status_storage.count_nodes().await,
                        status_storage.count_alive_nodes().await,
                    ) {
                        info!("Status: {} total nodes, {} alive", total, alive);
                        metrics::set_unique_nodes(total as u64);
                        metrics::set_alive_nodes(alive as u64);
                    }
                }
                _ = async {
                    let mut rx = status_stop_rx.clone();
                    let _ = rx.changed().await;
                } => {
                    if *status_stop_rx.borrow() {
                        break;
                    }
                }
            }
        }
    });

    // Wait for Ctrl+C
    tokio::signal::ctrl_c().await?;
    info!("Received Ctrl+C, shutting down...");
    let _ = stop_tx.send(true);

    // Wait for all tasks to finish
    let _ = crawler_handle.await;
    let _ = rechecker_handle.await;
    let _ = status_handle.await;
    drop(stop_tx);
    let count = storage_handle.await?;

    let total = storage.count_nodes().await?;
    let alive = storage.count_alive_nodes().await?;
    info!(
        "Shutdown complete. Discovered {} new nodes, {} total ({} alive)",
        count, total, alive
    );

    Ok(())
}

async fn run_crawl(
    storage: &Storage,
    chainspec: Arc<ChainSpec>,
    bootnodes: Vec<NodeRecord>,
    workers: usize,
    duration: Option<Duration>,
) -> eyre::Result<()> {
    let geoip = GeoIpResolver::new(None)?;
    let config = CrawlerConfig { chainspec, bootnodes, workers };
    let crawler = Crawler::new(config, geoip);

    let (tx, mut rx) = tokio::sync::mpsc::channel(1000);
    let (stop_tx, stop_rx) = tokio::sync::watch::channel(false);

    let storage_handle = {
        let storage = storage.clone();
        tokio::spawn(async move {
            let mut count = 0u64;
            while let Some(node) = rx.recv().await {
                if let Err(e) = storage.upsert_node(&node).await {
                    tracing::error!("Failed to store node: {}", e);
                } else {
                    count += 1;
                    if count.is_multiple_of(100) && storage.count_nodes().await.is_ok_and(|total| {
                        metrics::set_unique_nodes(total as u64);
                        info!("Stored {} nodes ({} unique)", count, total);
                        true
                    }) {}
                }
            }
            count
        })
    };

    let crawler_handle = tokio::spawn(async move { crawler.run(tx, stop_rx).await });

    if let Some(dur) = duration {
        tokio::time::sleep(dur).await;
    } else {
        tokio::signal::ctrl_c().await?;
        info!("Received Ctrl+C, shutting down...");
    }
    let _ = stop_tx.send(true);

    let _ = crawler_handle.await;
    drop(stop_tx);
    let count = storage_handle.await?;

    let total = storage.count_nodes().await?;
    info!("Crawl complete. Stored {} new nodes, {} total unique nodes", count, total);

    Ok(())
}

impl Clone for Storage {
    fn clone(&self) -> Self {
        Self { pool: self.pool.clone() }
    }
}
