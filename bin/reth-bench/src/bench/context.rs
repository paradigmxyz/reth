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
use tracing::info;

/// This is intended to be used by benchmarks that replay blocks from an RPC.
///
/// It contains an authenticated provider for engine API queries, a block provider for block
/// queries, a [`BenchMode`] to determine whether the benchmark should run for a closed or open
/// range of blocks, and the next block to fetch.
pub(crate) struct BenchContext {
    /// The auth provider is used for engine API queries.
    pub(crate) auth_provider: RootProvider<AnyNetwork>,
    /// The block provider is used for block queries if we are using an RPC as the source of
    /// blocks.
    pub(crate) block_provider: Option<RootProvider<AnyNetwork>>,
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
    pub(crate) async fn new(
        bench_args: &BenchmarkArgs,
        rpc_url: Option<String>,
    ) -> eyre::Result<Self> {
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

        // Set up client if using RPC
        let block_provider = if let Some(ref rpc_url) = rpc_url {
            info!("Setting up block provider for RPC: {}", rpc_url);
            let client = ClientBuilder::default()
                .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
                .http(rpc_url.parse()?);
            Some(RootProvider::<AnyNetwork>::new(client))
        } else {
            None
        };

        // Determine if this is Optimism
        let is_optimism = if let Some(file_path) = &bench_args.from_file {
            // Detect from file
            info!("Reading blocks from file: {:?}", file_path);
            let is_op = crate::bench::block_storage::detect_optimism_from_file(file_path)?;
            is_op
        } else {
            // Check if this is an OP chain by checking code at a predeploy address
            let is_op = !block_provider
                .as_ref()
                .unwrap()
                .get_code_at(address!("0x420000000000000000000000000000000000000F"))
                .await?
                .is_empty();
            is_op
        };

        // Construct the authenticated provider
        let auth_jwt = bench_args
            .auth_jwtsecret
            .clone()
            .ok_or_else(|| eyre::eyre!("--jwt-secret must be provided for authenticated RPC"))?;

        // Fetch jwt from file (hex encoded)
        let jwt = std::fs::read_to_string(auth_jwt)?;
        let jwt = JwtSecret::from_hex(jwt)?;

        // Get engine url
        let auth_url = Url::parse(&bench_args.engine_rpc_url)?;

        // Construct the authed transport
        info!("Connecting to Engine RPC at {} for replay", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(client);

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

            let head_block = auth_provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await?
                .ok_or_else(|| eyre::eyre!("Failed to fetch latest block for --advance"))?;
            let head_number = head_block.header.number;
            (Some(head_number), Some(head_number + advance))
        } else {
            (bench_args.from, bench_args.to)
        };

        // If neither `--from` nor `--to` are provided, we will run the benchmark continuously,
        // starting at the latest block.
        let mut benchmark_mode = BenchMode::new(from, to)?;

        // Only fetch first block if doing RPC benchmark
        let next_block = if let Some(ref provider) = block_provider {
            match benchmark_mode {
                BenchMode::Continuous => {
                    // Fetch latest block
                    provider
                        .get_block_by_number(BlockNumberOrTag::Latest)
                        .full()
                        .await?
                        .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?
                        .header
                        .number +
                        1
                }
                BenchMode::Range(ref mut range) => {
                    match range.next() {
                        Some(block_number) => {
                            // Fetch first block in range
                            provider
                                .get_block_by_number(block_number.into())
                                .full()
                                .await?
                                .ok_or_else(|| {
                                    eyre::eyre!("Failed to fetch block {}", block_number)
                                })?
                                .header
                                .number +
                                1
                        }
                        None => {
                            return Err(eyre::eyre!(
                                "Benchmark mode range is empty, please provide a larger range"
                            ));
                        }
                    }
                }
            }
        } else {
            // For file-based benchmarks, next_block isn't used by the fetching logic
            // Set to 0 as a sentinel value
            0
        };

        Ok(Self { auth_provider, block_provider, benchmark_mode, next_block, is_optimism })
    }
}
