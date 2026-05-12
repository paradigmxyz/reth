//! This contains the [`BenchContext`], which is information that all replay-based benchmarks need.
//! The initialization code is also the same, so this can be shared across benchmark commands.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::generate_big_block::BigBlocksInitialState, bench_mode::BenchMode,
};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::JwtSecret;
use alloy_transport::layers::{RateLimitRetryPolicy, RetryBackoffLayer};
use futures::{stream, StreamExt, TryStreamExt};
use reqwest::Url;
use reth_node_core::args::{BenchmarkArgs, WaitForPersistence};
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
    /// The local regular RPC provider is used for non-authenticated node RPCs like `testing_*`.
    pub(crate) local_rpc_provider: RootProvider<AnyNetwork>,
    /// The benchmark mode, which defines whether the benchmark should run for a closed or open
    /// range of blocks.
    pub(crate) benchmark_mode: BenchMode,
    /// The next block to fetch.
    pub(crate) next_block: u64,
    /// Whether to use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    pub(crate) use_reth_namespace: bool,
    /// Whether to fetch and replay RLP-encoded blocks.
    pub(crate) rlp_blocks: bool,
    /// Controls when `reth_newPayload` waits for persistence.
    pub(crate) wait_for_persistence: WaitForPersistence,
    /// Whether to skip waiting for caches (pass `wait_for_caches: false`).
    pub(crate) no_wait_for_caches: bool,
    /// Initial state for generated big blocks.
    pub(crate) big_blocks_initial_state: Option<BigBlocksInitialState>,
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

        // set up alloy client for blocks, retrying on any errors, whether HTTP or OS
        let retry_policy = RateLimitRetryPolicy::default().or(|_| true);
        let max_retries = bench_args.rpc_block_fetch_retries.as_max_retries();
        let client = ClientBuilder::default()
            .layer(RetryBackoffLayer::new_with_policy(max_retries, 800, u64::MAX, retry_policy))
            .http(rpc_url.parse()?);
        let block_provider = RootProvider::<AnyNetwork>::new(client);

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

        let local_rpc_url = Url::parse(&bench_args.local_rpc_url)?;
        info!(target: "reth-bench", "Connecting to local regular RPC at {} for testing namespace calls", local_rpc_url);
        let local_rpc_provider =
            RootProvider::<AnyNetwork>::new(ClientBuilder::default().http(local_rpc_url));

        // Computes the block range for the benchmark.
        //
        // - If `--advance` is provided, fetches the latest block from the engine and sets:
        //     - `from = head + 1`
        //     - `to = head + advance`
        // - If only `--to` is provided, fetches the latest block from the engine and sets:
        //     - `from = head`
        // - Otherwise, uses the values from `--from` and `--to`.
        let mut big_blocks_initial_state = None;
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
        } else if bench_args.big_blocks.is_some() && bench_args.from.is_none() {
            let (from, initial_state) =
                derive_big_blocks_initial_state(&auth_provider, &block_provider).await?;
            big_blocks_initial_state = initial_state;

            (Some(from), bench_args.to)
        } else if bench_args.from.is_none() && bench_args.to.is_some() {
            let head_block = auth_provider
                .get_block_by_number(BlockNumberOrTag::Latest)
                .await?
                .ok_or_else(|| eyre::eyre!("Failed to fetch latest block from engine"))?;
            let head_number = head_block.header.number;
            info!(target: "reth-bench", "No --from provided, derived from engine head: {}", head_number);
            (Some(head_number), bench_args.to)
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
        let mut benchmark_mode = BenchMode::new(from, to, latest_block.into_inner().number());

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
        let wait_for_persistence =
            bench_args.wait_for_persistence.unwrap_or(WaitForPersistence::Never);
        let use_reth_namespace = bench_args.reth_new_payload || rlp_blocks;
        let no_wait_for_caches = bench_args.no_wait_for_caches;
        Ok(Self {
            auth_provider,
            block_provider,
            local_rpc_provider,
            benchmark_mode,
            next_block,
            use_reth_namespace,
            rlp_blocks,
            wait_for_persistence,
            no_wait_for_caches,
            big_blocks_initial_state,
        })
    }
}

/// Derives the initial state for big blocks benchmark from RPC of the local node.
async fn derive_big_blocks_initial_state(
    local_provider: &RootProvider<AnyNetwork>,
    source_provider: &RootProvider<AnyNetwork>,
) -> eyre::Result<(u64, Option<BigBlocksInitialState>)> {
    let local_head = local_provider
        .get_block_by_number(BlockNumberOrTag::Latest)
        .full()
        .await?
        .ok_or_else(|| eyre::eyre!("Failed to fetch latest block from engine"))?;

    let local_head_number = local_head.header.number;
    let local_head_hash = local_head.header.hash;

    let source_block_at_local_head = source_provider
        .get_block_by_number(local_head_number.into())
        .await?
        .ok_or_else(|| eyre::eyre!("Failed to fetch block {local_head_number} from RPC"))?;

    // Node's tip is not synthetic, no initial state needed
    if source_block_at_local_head.header.number == local_head_number &&
        source_block_at_local_head.header.hash == local_head_hash
    {
        return Ok((local_head_number, None));
    }

    // If the tip is synthetic, derive last regular block from the last transaction the node has.
    let last_regular_block = if let Some(tx_hash) = local_head.transactions.hashes().last() {
        let tx = source_provider
            .get_transaction_by_hash(tx_hash)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch transaction {tx_hash} from RPC"))?;
        tx.block_number
            .ok_or_else(|| eyre::eyre!("Transaction {tx_hash} from local head is pending on RPC"))?
    } else {
        return Err(eyre::eyre!(
            "Synthetic local tip has no transactions, can't derive last regular block"
        ));
    };

    let initial_state = BigBlocksInitialState {
        prior_block_hashes: fetch_recent_block_hashes(source_provider, last_regular_block).await?,
        next_synthetic_block_number: local_head_number + 1,
    };

    Ok((last_regular_block, Some(initial_state)))
}

async fn fetch_recent_block_hashes(
    provider: &RootProvider<AnyNetwork>,
    latest_regular_block: u64,
) -> eyre::Result<Vec<(u64, B256)>> {
    const BLOCKHASH_HISTORY: u64 = 256;
    const MAX_CONCURRENT_BLOCK_HASH_REQUESTS: usize = 5;

    let start = latest_regular_block.saturating_sub(BLOCKHASH_HISTORY - 1);
    let hashes = stream::iter(start..=latest_regular_block)
        .map(|block_number| async move {
            provider
                .get_block_by_number(block_number.into())
                .await
                .map(|block| block.map(|block| (block_number, block.header.hash)))
        })
        .buffered(MAX_CONCURRENT_BLOCK_HASH_REQUESTS)
        .try_filter_map(|block_hash| async move { Ok(block_hash) })
        .try_collect()
        .await?;

    Ok(hashes)
}
