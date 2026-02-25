//! Benchmarks empty block processing by ramping the block gas limit.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::{
        helpers::{build_payload, parse_gas_limit, prepare_payload_request, rpc_block_to_header},
        output::GasRampPayloadFile,
    },
    valid_payload::{
        call_forkchoice_updated_with_reth, call_new_payload_with_reth, payload_to_new_payload,
    },
};
use alloy_eips::BlockNumberOrTag;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ExecutionPayload, ForkchoiceState, JwtSecret};

use clap::Parser;
use reqwest::Url;
use reth_chainspec::ChainSpec;
use reth_cli_runner::CliContext;
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::constants::{GAS_LIMIT_BOUND_DIVISOR, MAXIMUM_GAS_LIMIT_BLOCK};
use reth_rpc_api::RethNewPayloadInput;
use std::{path::PathBuf, time::Instant};
use tracing::info;

/// `reth benchmark gas-limit-ramp` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// Number of blocks to generate. Mutually exclusive with --target-gas-limit.
    #[arg(long, value_name = "BLOCKS", conflicts_with = "target_gas_limit")]
    blocks: Option<u64>,

    /// Target gas limit to ramp up to. The benchmark will generate blocks until the gas limit
    /// reaches or exceeds this value. Mutually exclusive with --blocks.
    /// Accepts short notation: K for thousand, M for million, G for billion (e.g., 2G = 2
    /// billion).
    #[arg(long, value_name = "TARGET_GAS_LIMIT", conflicts_with = "blocks", value_parser = parse_gas_limit)]
    target_gas_limit: Option<u64>,

    /// The Engine API RPC URL.
    #[arg(long = "engine-rpc-url", value_name = "ENGINE_RPC_URL")]
    engine_rpc_url: String,

    /// Path to the JWT secret for Engine API authentication.
    #[arg(long = "jwt-secret", value_name = "JWT_SECRET")]
    jwt_secret: PathBuf,

    /// Output directory for benchmark results and generated payloads.
    #[arg(long, value_name = "OUTPUT")]
    output: PathBuf,

    /// Use `reth_newPayload` endpoint instead of `engine_newPayload*`.
    ///
    /// The `reth_newPayload` endpoint is a reth-specific extension that takes `ExecutionData`
    /// directly, waits for persistence and cache updates to complete before processing,
    /// and returns server-side timing breakdowns (latency, persistence wait, cache wait).
    #[arg(long, default_value = "false", verbatim_doc_comment)]
    reth_new_payload: bool,
}

/// Mode for determining when to stop ramping.
#[derive(Debug, Clone, Copy)]
enum RampMode {
    /// Ramp for a fixed number of blocks.
    Blocks(u64),
    /// Ramp until reaching or exceeding target gas limit.
    TargetGasLimit(u64),
}

impl Command {
    /// Execute `benchmark gas-limit-ramp` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        let mode = match (self.blocks, self.target_gas_limit) {
            (Some(blocks), None) => {
                if blocks == 0 {
                    return Err(eyre::eyre!("--blocks must be greater than 0"));
                }
                RampMode::Blocks(blocks)
            }
            (None, Some(target)) => {
                if target == 0 {
                    return Err(eyre::eyre!("--target-gas-limit must be greater than 0"));
                }
                RampMode::TargetGasLimit(target)
            }
            _ => {
                return Err(eyre::eyre!(
                    "Exactly one of --blocks or --target-gas-limit must be specified"
                ));
            }
        };

        // Ensure output directory exists
        if self.output.is_file() {
            return Err(eyre::eyre!("Output path must be a directory"));
        }
        if !self.output.exists() {
            std::fs::create_dir_all(&self.output)?;
            info!(target: "reth-bench", "Created output directory: {:?}", self.output);
        }

        // Set up authenticated provider (used for both Engine API and eth_ methods)
        let jwt = std::fs::read_to_string(&self.jwt_secret)?;
        let jwt = JwtSecret::from_hex(jwt)?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!(target: "reth-bench", "Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_with(auth_transport).await?;
        let provider = RootProvider::<AnyNetwork>::new(client);

        // Get chain spec - required for fork detection
        let chain_id = provider.get_chain_id().await?;
        let chain_spec = ChainSpec::from_chain_id(chain_id)
            .ok_or_else(|| eyre::eyre!("Unsupported chain id: {chain_id}"))?;

        // Fetch the current head block as parent
        let parent_block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        let (mut parent_header, mut parent_hash) = rpc_block_to_header(parent_block);

        let canonical_parent = parent_header.number;
        let start_block = canonical_parent + 1;

        match mode {
            RampMode::Blocks(blocks) => {
                info!(
                    target: "reth-bench",
                    canonical_parent,
                    start_block,
                    end_block = start_block + blocks - 1,
                    "Starting gas limit ramp benchmark (block count mode)"
                );
            }
            RampMode::TargetGasLimit(target) => {
                info!(
                    target: "reth-bench",
                    canonical_parent,
                    start_block,
                    current_gas_limit = parent_header.gas_limit,
                    target_gas_limit = target,
                    "Starting gas limit ramp benchmark (target gas limit mode)"
                );
            }
        }
        if self.reth_new_payload {
            info!("Using reth_newPayload and reth_forkchoiceUpdated endpoints");
        }

        let mut blocks_processed = 0u64;
        let total_benchmark_duration = Instant::now();

        while !should_stop(mode, blocks_processed, parent_header.gas_limit) {
            let timestamp = parent_header.timestamp.saturating_add(1);

            let request = prepare_payload_request(&chain_spec, timestamp, parent_hash);
            let new_payload_version = request.new_payload_version;

            let (payload, sidecar) = build_payload(&provider, request).await?;

            let mut block =
                payload.clone().try_into_block_with_sidecar::<TransactionSigned>(&sidecar)?;

            let max_increase = max_gas_limit_increase(parent_header.gas_limit);
            let gas_limit =
                parent_header.gas_limit.saturating_add(max_increase).min(MAXIMUM_GAS_LIMIT_BLOCK);

            block.header.gas_limit = gas_limit;

            let block_hash = block.header.hash_slow();
            // Regenerate the payload from the modified block, but keep the original sidecar
            // which contains the actual execution requests data (not just the hash)
            let (payload, _) = ExecutionPayload::from_block_unchecked(block_hash, &block);
            let (version, params, execution_data) = payload_to_new_payload(
                payload,
                sidecar,
                false,
                block.header.withdrawals_root,
                Some(new_payload_version),
            )?;

            let (version, params) = if self.reth_new_payload {
                (None, serde_json::to_value((RethNewPayloadInput::ExecutionData(execution_data),))?)
            } else {
                (Some(version), params)
            };

            // Save payload to file with version info for replay
            let payload_path =
                self.output.join(format!("payload_block_{}.json", block.header.number));
            let file = GasRampPayloadFile {
                version: version.map(|v| v as u8),
                block_hash,
                params: params.clone(),
            };
            let payload_json = serde_json::to_string_pretty(&file)?;
            std::fs::write(&payload_path, &payload_json)?;
            info!(target: "reth-bench", block_number = block.header.number, path = %payload_path.display(), "Saved payload");

            let _ = call_new_payload_with_reth(&provider, version, params).await?;

            let forkchoice_state = ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: block_hash,
                finalized_block_hash: block_hash,
            };
            call_forkchoice_updated_with_reth(&provider, version, forkchoice_state).await?;

            parent_header = block.header;
            parent_hash = block_hash;
            blocks_processed += 1;

            let progress = match mode {
                RampMode::Blocks(total) => format!("{blocks_processed}/{total}"),
                RampMode::TargetGasLimit(target) => {
                    let pct = (parent_header.gas_limit as f64 / target as f64 * 100.0).min(100.0);
                    format!("{pct:.1}%")
                }
            };
            info!(target: "reth-bench", progress, block_number = parent_header.number, gas_limit = parent_header.gas_limit, "Block processed");
        }

        let final_gas_limit = parent_header.gas_limit;
        info!(
            target: "reth-bench",
            total_duration=?total_benchmark_duration.elapsed(),
            blocks_processed,
            final_gas_limit,
            "Benchmark complete"
        );

        Ok(())
    }
}

const fn max_gas_limit_increase(parent_gas_limit: u64) -> u64 {
    (parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR).saturating_sub(1)
}

const fn should_stop(mode: RampMode, blocks_processed: u64, current_gas_limit: u64) -> bool {
    match mode {
        RampMode::Blocks(target_blocks) => blocks_processed >= target_blocks,
        RampMode::TargetGasLimit(target) => current_gas_limit >= target,
    }
}
