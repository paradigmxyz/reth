//! Benchmarks empty block processing by ramping the block gas limit.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    bench::{
        helpers::{build_payload, prepare_payload_request, rpc_block_to_header},
        output::{
            write_benchmark_results, CombinedResult, NewPayloadResult, TotalGasOutput, TotalGasRow,
        },
    },
    valid_payload::{call_forkchoice_updated, call_new_payload, payload_to_new_payload},
};
use alloy_eips::BlockNumberOrTag;
use alloy_provider::{network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ExecutionPayload, ForkchoiceState, JwtSecret};

use clap::Parser;
use reqwest::Url;
use reth_chainspec::{ChainSpec, NamedChain, DEV, HOLESKY, HOODI, MAINNET, SEPOLIA};
use reth_cli_runner::CliContext;
use reth_ethereum_primitives::TransactionSigned;
use reth_primitives_traits::constants::{GAS_LIMIT_BOUND_DIVISOR, MAXIMUM_GAS_LIMIT_BLOCK};
use std::{path::PathBuf, sync::Arc, time::Instant};
use tracing::{debug, info};

/// `reth benchmark gas-limit-ramp` command.
#[derive(Debug, Parser)]
pub struct Command {
    /// Number of blocks to generate.
    #[arg(long, value_name = "BLOCKS")]
    blocks: u64,

    /// The Engine API RPC URL.
    #[arg(long = "engine-rpc-url", value_name = "ENGINE_RPC_URL")]
    engine_rpc_url: String,

    /// Path to the JWT secret for Engine API authentication.
    #[arg(long = "jwt-secret", value_name = "JWT_SECRET")]
    jwt_secret: PathBuf,

    /// Output directory for benchmark results and generated payloads.
    #[arg(long, value_name = "OUTPUT")]
    output: PathBuf,
}

/// Map a chain ID to a known chain spec.
fn chain_spec_from_id(chain_id: u64) -> Option<Arc<ChainSpec>> {
    match NamedChain::try_from(chain_id).ok()? {
        NamedChain::Mainnet => Some(MAINNET.clone()),
        NamedChain::Sepolia => Some(SEPOLIA.clone()),
        NamedChain::Holesky => Some(HOLESKY.clone()),
        NamedChain::Hoodi => Some(HOODI.clone()),
        NamedChain::Dev => Some(DEV.clone()),
        _ => None,
    }
}

impl Command {
    /// Execute `benchmark gas-limit-ramp` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        if self.blocks == 0 {
            return Err(eyre::eyre!("--blocks must be greater than 0"));
        }

        // Ensure output directory exists
        if self.output.is_file() {
            return Err(eyre::eyre!("Output path must be a directory"));
        }
        if !self.output.exists() {
            std::fs::create_dir_all(&self.output)?;
            info!("Created output directory: {:?}", self.output);
        }

        // Set up authenticated provider (used for both Engine API and eth_ methods)
        let jwt = std::fs::read_to_string(&self.jwt_secret)?;
        let jwt = JwtSecret::from_hex(jwt)?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!("Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url, jwt);
        let client = ClientBuilder::default().connect_with(auth_transport).await?;
        let provider = RootProvider::<AnyNetwork>::new(client);

        // Get chain spec - required for fork detection
        let chain_id = provider.get_chain_id().await?;
        let chain_spec = chain_spec_from_id(chain_id)
            .ok_or_else(|| eyre::eyre!("Unsupported chain id: {chain_id}"))?;

        // Fetch the current head block as parent
        let parent_block = provider
            .get_block_by_number(BlockNumberOrTag::Latest)
            .full()
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        debug!(?parent_block, "Raw RPC parent block");

        let (mut parent_header, mut parent_hash) = rpc_block_to_header(parent_block);

        let start_block = parent_header.number + 1;
        let end_block = start_block + self.blocks - 1;

        debug!(?parent_header, "Converted parent header");

        info!(start_block, end_block, "Starting gas limit ramp benchmark");

        let mut next_block_number = start_block;
        let mut results = Vec::new();
        let total_benchmark_duration = Instant::now();

        while next_block_number <= end_block {
            let timestamp = parent_header.timestamp.saturating_add(1);

            let request = prepare_payload_request(&chain_spec, timestamp, parent_hash);
            let new_payload_version = request.new_payload_version;

            let (payload, sidecar) = build_payload(&provider, request).await?;

            let mut block =
                payload.clone().try_into_block_with_sidecar::<TransactionSigned>(&sidecar)?;

            let max_increase = max_gas_limit_increase(parent_header.gas_limit);
            let gas_limit = next_gas_limit(parent_header.gas_limit, max_increase);

            block.header.gas_limit = gas_limit;

            let block_hash = block.header.hash_slow();
            // Regenerate the payload from the modified block, but keep the original sidecar
            // which contains the actual execution requests data (not just the hash)
            let (payload, _) = ExecutionPayload::from_block_unchecked(block_hash, &block);
            let (version, params) = payload_to_new_payload(
                payload,
                sidecar,
                false,
                block.header.withdrawals_root,
                Some(new_payload_version),
            )?;

            // Save payload to file with version info for replay
            let payload_path =
                self.output.join(format!("payload_block_{}.json", block.header.number));
            let wrapper = serde_json::json!({
                "version": version as u8,
                "block_hash": block_hash,
                "params": params,
            });
            let payload_json = serde_json::to_string_pretty(&wrapper)?;
            std::fs::write(&payload_path, &payload_json)?;
            info!(block_number = block.header.number, path = %payload_path.display(), "Saved payload");

            debug!(
                target: "reth-bench",
                block_number = block.header.number,
                gas_limit,
                max_increase,
                "Sending empty payload"
            );

            let start = Instant::now();
            call_new_payload(&provider, version, params).await?;

            let new_payload_result =
                NewPayloadResult { gas_used: block.header.gas_used, latency: start.elapsed() };

            let forkchoice_state = ForkchoiceState {
                head_block_hash: block_hash,
                safe_block_hash: block_hash,
                finalized_block_hash: block_hash,
            };
            call_forkchoice_updated(&provider, version, forkchoice_state, None).await?;

            let total_latency = start.elapsed();
            let fcu_latency = total_latency - new_payload_result.latency;
            let transaction_count = block.body.transactions.len() as u64;
            let combined_result = CombinedResult {
                block_number: block.header.number,
                gas_limit,
                transaction_count,
                new_payload_result,
                fcu_latency,
                total_latency,
            };

            info!(%combined_result);

            let gas_row = TotalGasRow {
                block_number: block.header.number,
                transaction_count,
                gas_used: block.header.gas_used,
                time: total_benchmark_duration.elapsed(),
            };
            results.push((gas_row, combined_result));

            parent_header = block.header;
            parent_hash = block_hash;

            debug!(?parent_header, "RPC parent header");
            next_block_number += 1;
        }

        let (gas_output_results, combined_results): (Vec<TotalGasRow>, Vec<CombinedResult>) =
            results.into_iter().unzip();

        write_benchmark_results(&self.output, &gas_output_results, combined_results)?;

        let final_gas_limit = parent_header.gas_limit;
        let gas_output = TotalGasOutput::new(gas_output_results)?;
        info!(
            total_duration=?gas_output.total_duration,
            total_gas_used=?gas_output.total_gas_used,
            blocks_processed=?gas_output.blocks_processed,
            final_gas_limit,
            "Total Ggas/s: {:.4}",
            gas_output.total_gigagas_per_second()
        );

        Ok(())
    }
}

const fn max_gas_limit_increase(parent_gas_limit: u64) -> u64 {
    (parent_gas_limit / GAS_LIMIT_BOUND_DIVISOR).saturating_sub(1)
}

fn next_gas_limit(parent_gas_limit: u64, max_increase: u64) -> u64 {
    parent_gas_limit.saturating_add(max_increase).min(MAXIMUM_GAS_LIMIT_BLOCK)
}
