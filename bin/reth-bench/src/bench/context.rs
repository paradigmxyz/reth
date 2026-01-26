//! This contains the [`BenchContext`], which is information that all replay-based benchmarks need.
//! The initialization code is also the same, so this can be shared across benchmark commands.

use crate::{authenticated_transport::AuthenticatedTransportConnect, bench_mode::BenchMode};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::address;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::layers::RetryBackoffLayer;
use reqwest::Url;
use reth_node_core::args::BenchmarkArgs;
use std::path::PathBuf;
use tracing::info;

/// This is intended to be used by benchmarks that replay blocks from an RPC.
///
/// It contains authenticated providers for engine API queries (one per engine URL),
/// a block provider for block queries, a [`BenchMode`] to determine whether the benchmark
/// should run for a closed or open range of blocks, and the next block to fetch.
pub(crate) struct BenchContext {
    /// The auth providers are used for engine API queries (one per engine URL).
    pub(crate) auth_providers: Vec<RootProvider<AnyNetwork>>,
    /// The block provider is used for block queries.
    pub(crate) block_provider: RootProvider<AnyNetwork>,
    /// The benchmark mode, which defines whether the benchmark should run for a closed or open
    /// range of blocks.
    pub(crate) benchmark_mode: BenchMode,
    /// The next block to fetch.
    pub(crate) next_block: u64,
    /// Whether the chain is an OP rollup.
    pub(crate) is_optimism: bool,
}

impl BenchContext {
    /// This is the initialization code for most benchmarks, taking in a [`BenchmarkArgs`] and
    /// returning the providers needed to run a benchmark.
    pub(crate) async fn new(bench_args: &BenchmarkArgs, rpc_url: String) -> eyre::Result<Self> {
        info!("Running benchmark using data from RPC URL: {}", rpc_url);

        // Ensure that output directory exists and is a directory
        if let Some(output) = &bench_args.output {
            if output.is_file() {
                return Err(eyre::eyre!("Output path must be a directory"));
            }
            // Create the directory if it doesn't exist
            if !output.exists() {
                std::fs::create_dir_all(output)?;
                info!("Created output directory: {:?}", output);
            }
        }

        // set up alloy client for blocks
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(rpc_url.parse()?);
        let block_provider = RootProvider::<AnyNetwork>::new(client);

        // Check if this is an OP chain by checking code at a predeploy address.
        let is_optimism = !block_provider
            .get_code_at(address!("0x420000000000000000000000000000000000000F"))
            .await?
            .is_empty();

        // Build the list of (engine_url, jwt_secret) pairs
        let engine_jwt_pairs =
            build_engine_jwt_pairs(&bench_args.engine_rpc_url, &bench_args.auth_jwtsecret)?;

        // Create authenticated providers for each engine URL
        let mut auth_providers = Vec::with_capacity(engine_jwt_pairs.len());
        for (engine_url, jwt_path) in &engine_jwt_pairs {
            let jwt = std::fs::read_to_string(jwt_path)?;
            let jwt = JwtSecret::from_hex(jwt)?;
            let auth_url = Url::parse(engine_url)?;

            info!("Connecting to Engine RPC at {} for replay", auth_url);
            let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
            let client = ClientBuilder::default().connect_with(auth_transport).await?;
            let auth_provider = RootProvider::<AnyNetwork>::new(client);
            auth_providers.push(auth_provider);
        }

        // Verify all nodes are at the same height
        verify_nodes_same_height(&auth_providers).await?;

        // Use the first auth provider for block range calculations
        let first_auth_provider = auth_providers
            .first()
            .ok_or_else(|| eyre::eyre!("At least one engine RPC URL is required"))?;

        // Computes the block range for the benchmark.
        //
        // - If `--advance` is provided, fetches the latest block and sets:
        //     - `from = head + 1`
        //     - `to = head + advance`
        // - Otherwise, uses the values from `--from` and `--to`.
        let (from, to) = if let Some(advance) = bench_args.advance {
            if advance == 0 {
                return Err(eyre::eyre!("--advance must be greater than 0"));
            }

            let head_block = first_auth_provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await?
                .ok_or_else(|| eyre::eyre!("Failed to fetch latest block for --advance"))?;
            let head_number = head_block.header.number;
            (Some(head_number), Some(head_number + advance))
        } else {
            (bench_args.from, bench_args.to)
        };

        // If `--to` are not provided, we will run the benchmark continuously,
        // starting at the latest block.
        let latest_block = block_provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block from RPC"))?;
        let mut benchmark_mode = BenchMode::new(from, to, latest_block.into_inner().number())?;

        let first_block = match benchmark_mode {
            BenchMode::Continuous(start) => {
                block_provider.get_block_by_number(start.into()).full().await?.ok_or_else(|| {
                    eyre::eyre!("Failed to fetch block {} from RPC for continuous mode", start)
                })?
            }
            BenchMode::Range(ref mut range) => {
                match range.next() {
                    Some(block_number) => {
                        // fetch first block in range
                        block_provider
                            .get_block_by_number(block_number.into())
                            .full()
                            .await?
                            .ok_or_else(|| {
                                eyre::eyre!("Failed to fetch block {} from RPC", block_number)
                            })?
                    }
                    None => {
                        return Err(eyre::eyre!(
                            "Benchmark mode range is empty, please provide a larger range"
                        ));
                    }
                }
            }
        };

        let next_block = first_block.header.number + 1;
        Ok(Self { auth_providers, block_provider, benchmark_mode, next_block, is_optimism })
    }
}

/// Build pairs of (`engine_url`, `jwt_path`) from the command line arguments.
///
/// Supports two modes:
/// - One JWT secret for all engine URLs (shared secret)
/// - One JWT secret per engine URL (1:1 mapping)
fn build_engine_jwt_pairs(
    engine_urls: &[String],
    jwt_paths: &[PathBuf],
) -> eyre::Result<Vec<(String, PathBuf)>> {
    if jwt_paths.is_empty() {
        return Err(eyre::eyre!("--jwt-secret must be provided for authenticated RPC"));
    }

    if jwt_paths.len() == 1 {
        // One JWT for all engine URLs
        let jwt_path = jwt_paths[0].clone();
        Ok(engine_urls.iter().map(|url| (url.clone(), jwt_path.clone())).collect())
    } else if jwt_paths.len() == engine_urls.len() {
        // 1:1 mapping
        Ok(engine_urls.iter().cloned().zip(jwt_paths.iter().cloned()).collect())
    } else {
        Err(eyre::eyre!(
            "Number of --jwt-secret ({}) must be 1 or match number of --engine-rpc-url ({})",
            jwt_paths.len(),
            engine_urls.len()
        ))
    }
}

/// Verify that all nodes are at the same block height.
async fn verify_nodes_same_height(providers: &[RootProvider<AnyNetwork>]) -> eyre::Result<()> {
    if providers.len() <= 1 {
        return Ok(());
    }

    info!("Verifying all {} nodes are at the same height...", providers.len());

    let mut heights = Vec::with_capacity(providers.len());
    for (i, provider) in providers.iter().enumerate() {
        let block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block from engine {}", i))?;
        heights.push((i, block.header.number, block.header.hash));
    }

    let first_height = heights[0].1;
    let first_hash = heights[0].2;

    for (i, height, hash) in &heights[1..] {
        if *height != first_height {
            return Err(eyre::eyre!(
                "Engine nodes are at different heights: engine 0 is at block {}, engine {} is at block {}. \
                 All nodes must be at the same height before benchmarking.",
                first_height, i, height
            ));
        }
        if *hash != first_hash {
            return Err(eyre::eyre!(
                "Engine nodes have different block hashes at height {}: engine 0 has {}, engine {} has {}. \
                 All nodes must be on the same chain.",
                first_height, first_hash, i, hash
            ));
        }
    }

    info!(
        "All {} nodes verified at height {} with hash {}",
        providers.len(),
        first_height,
        first_hash
    );

    Ok(())
}
