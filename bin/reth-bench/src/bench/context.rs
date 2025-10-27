//! This contains the [`BenchContext`], which is information that all replay-based benchmarks need.
//! The initialization code is also the same, so this can be shared across benchmark commands.

use std::path::PathBuf;

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::block_storage::{BlockFileReader, BlockType},
    bench_mode::BenchMode,
};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::address;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::layers::RetryBackoffLayer;
use eyre::OptionExt;
use reqwest::Url;
use reth_node_core::args::BenchmarkArgs;
use tracing::info;

/// Represents the source of blocks for benchmarking
pub(crate) enum BlockSource {
    /// Blocks are fetched from an RPC endpoint
    Rpc {
        /// The RPC provider for fetching blocks
        provider: RootProvider<AnyNetwork>,
        /// The next block number to process
        next_block: u64,
        /// The benchmark mode (range or continuous)
        mode: BenchMode,
    },
    /// Blocks are loaded from a file
    File {
        /// Path to the block file
        path: PathBuf,
    },
}

/// This is intended to be used by benchmarks that replay blocks from an RPC or a file.
///
/// It contains an authenticated provider for engine API queries,
/// the block source, and a [`BenchMode`] to determine whether the benchmark should run for a
/// closed, open, or file-based range of blocks.
pub(crate) struct BenchContext {
    /// The auth provider is used for engine API queries.
    pub(crate) auth_provider: RootProvider<AnyNetwork>,
    /// The source of blocks for the benchmark.
    pub(crate) block_source: BlockSource,
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

        // Construct the authenticated provider first (needed for --advance)
        let auth_jwt = bench_args
            .auth_jwtsecret
            .clone()
            .ok_or_else(|| eyre::eyre!("--jwt-secret must be provided for authenticated RPC"))?;

        // Fetch jwt from file (the jwt is hex encoded so we will decode it after)
        let jwt = std::fs::read_to_string(auth_jwt)?;
        let jwt = JwtSecret::from_hex(jwt)?;

        // Get engine url
        let auth_url = Url::parse(&bench_args.engine_rpc_url)?;

        // Construct the authed transport
        info!("Connecting to Engine RPC at {} for replay", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(client);

        // Determine block source, benchmark mode, and whether it's optimism - all in one place
        let (block_source, is_optimism) = if let Some(file_path) = &bench_args.from_file {
            // File-based loading
            info!("Running benchmark using data from file: {:?}", file_path);

            // Read block type from file header
            let file_header = BlockFileReader::get_header(file_path)?;
            let is_optimism = file_header.block_type() == BlockType::Optimism;

            let block_source = BlockSource::File { path: file_path.clone() };

            (block_source, is_optimism)
        } else {
            // RPC-based loading
            let rpc_url = rpc_url
                .ok_or_else(|| eyre::eyre!("Either --rpc-url or --from-file must be provided"))?;

            info!("Running benchmark using data from RPC URL: {}", rpc_url);

            let client = ClientBuilder::default()
                .layer(RetryBackoffLayer::new(10, 800, u64::MAX))
                .http(rpc_url.parse()?);
            let provider = RootProvider::<AnyNetwork>::new(client);

            // Check if this is an OP chain by checking code at a predeploy address
            let is_optimism = !provider
                .get_code_at(address!("0x420000000000000000000000000000000000000F"))
                .await?
                .is_empty();

            // Compute the block range for the benchmark
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

            let mode = BenchMode::new(from, to)?;

            // Determine next_block based on benchmark mode
            let next_block = match &mode {
                BenchMode::Continuous => {
                    let latest = provider
                        .get_block_by_number(BlockNumberOrTag::Latest)
                        .await?
                        .ok_or_eyre("Failed to fetch latest block")?;
                    latest.header.number + 1
                }
                BenchMode::Range(range) => *range.start() + 1,
            };

            let block_source = BlockSource::Rpc { provider, next_block, mode };

            (block_source, is_optimism)
        };

        Ok(Self { auth_provider, block_source, is_optimism })
    }
}
