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
    /// The block provider is used for block queries.
    pub(crate) block_provider: RootProvider<AnyNetwork>,
    /// The benchmark mode, which defines whether the benchmark should run for a closed or open
    /// range of blocks.
    pub(crate) benchmark_mode: BenchMode,
    /// The next block to fetch.
    pub(crate) next_block: u64,
    /// Whether the chain is an OP rollup.
    pub(crate) is_optimism: bool,
    /// Whether to use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    pub(crate) use_reth_namespace: bool,
    /// Whether to fetch and replay RLP-encoded blocks.
    pub(crate) rlp_blocks: bool,
}

impl BenchContext {
    /// This is the initialization code for most benchmarks, taking in a [`BenchmarkArgs`] and
    /// returning the providers needed to run a benchmark.
    pub(crate) async fn new(bench_args: &BenchmarkArgs, rpc_url: String) -> eyre::Result<Self> {
        info!(target: "reth-bench", "Running benchmark using data from RPC URL: {}", rpc_url);

        // Ensure that output directory exists and is a directory
        if let Some(output) = &bench_args.output {
            if output.is_file() {
                return Err(eyre::eyre!("Output path must be a directory"));
            }
            // Create the directory if it doesn't exist
            if !output.exists() {
                std::fs::create_dir_all(output)?;
                info!(target: "reth-bench", "Created output directory: {:?}", output);
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

        // construct the authenticated provider
        let auth_jwt = bench_args
            .auth_jwtsecret
            .clone()
            .ok_or_else(|| eyre::eyre!("--jwt-secret must be provided for authenticated RPC"))?;

        // fetch jwt from file
        //
        // the jwt is hex encoded so we will decode it after
        let jwt = std::fs::read_to_string(auth_jwt)?;
        let jwt = JwtSecret::from_hex(jwt)?;

        // get engine url
        let auth_url = Url::parse(&bench_args.engine_rpc_url)?;

        // construct the authed transport
        info!(target: "reth-bench", "Connecting to Engine RPC at {} for replay", auth_url);
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
        let rlp_blocks = bench_args.rlp_blocks;
        let use_reth_namespace = bench_args.reth_new_payload || rlp_blocks;
        Ok(Self {
            auth_provider,
            block_provider,
            benchmark_mode,
            next_block,
            is_optimism,
            use_reth_namespace,
            rlp_blocks,
        })
    }
}
