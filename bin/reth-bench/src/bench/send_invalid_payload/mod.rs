//! Command for sending invalid payloads to test Engine API rejection.

mod invalidation;
use invalidation::InvalidationConfig;

use super::helpers::{load_jwt_secret, read_input};
use alloy_primitives::{Address, B256};
use alloy_provider::network::AnyRpcBlock;
use alloy_rpc_types_engine::ExecutionPayload;
use clap::Parser;
use eyre::{OptionExt, Result};
use op_alloy_consensus::OpTxEnvelope;
use reth_cli_runner::CliContext;
use std::io::Write;

/// Command for generating and sending an invalid `engine_newPayload` request.
///
/// Takes a valid block and modifies fields to make it invalid for testing
/// Engine API rejection behavior. Block hash is recalculated after modifications
/// unless `--invalidate-block-hash` or `--skip-hash-recalc` is used.
#[derive(Debug, Parser)]
pub struct Command {
    // ==================== Input Options ====================
    /// Path to the JSON file containing the block. If not specified, stdin will be used.
    #[arg(short, long, help_heading = "Input Options")]
    path: Option<String>,

    /// The engine RPC URL to use.
    #[arg(
        short,
        long,
        help_heading = "Input Options",
        required_if_eq_any([("mode", "execute"), ("mode", "cast")]),
        required_unless_present("mode")
    )]
    rpc_url: Option<String>,

    /// The JWT secret to use. Can be either a path to a file containing the secret or the secret
    /// itself.
    #[arg(short, long, help_heading = "Input Options")]
    jwt_secret: Option<String>,

    /// The newPayload version to use (3 or 4).
    #[arg(long, default_value_t = 3, help_heading = "Input Options")]
    new_payload_version: u8,

    /// The output mode to use.
    #[arg(long, value_enum, default_value = "execute", help_heading = "Input Options")]
    mode: Mode,

    // ==================== Explicit Value Overrides ====================
    /// Override the parent hash with a specific value.
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    parent_hash: Option<B256>,

    /// Override the fee recipient (coinbase) with a specific address.
    #[arg(long, value_name = "ADDR", help_heading = "Explicit Value Overrides")]
    fee_recipient: Option<Address>,

    /// Override the state root with a specific value.
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    state_root: Option<B256>,

    /// Override the receipts root with a specific value.
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    receipts_root: Option<B256>,

    /// Override the block number with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    block_number: Option<u64>,

    /// Override the gas limit with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    gas_limit: Option<u64>,

    /// Override the gas used with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    gas_used: Option<u64>,

    /// Override the timestamp with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    timestamp: Option<u64>,

    /// Override the base fee per gas with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    base_fee_per_gas: Option<u64>,

    /// Override the block hash with a specific value (skips hash recalculation).
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    block_hash: Option<B256>,

    /// Override the blob gas used with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    blob_gas_used: Option<u64>,

    /// Override the excess blob gas with a specific value.
    #[arg(long, value_name = "U64", help_heading = "Explicit Value Overrides")]
    excess_blob_gas: Option<u64>,

    /// Override the parent beacon block root with a specific value.
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    parent_beacon_block_root: Option<B256>,

    /// Override the requests hash with a specific value (EIP-7685).
    #[arg(long, value_name = "HASH", help_heading = "Explicit Value Overrides")]
    requests_hash: Option<B256>,

    // ==================== Auto-Invalidation Flags ====================
    /// Invalidate the parent hash by setting it to a random value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_parent_hash: bool,

    /// Invalidate the state root by setting it to a random value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_state_root: bool,

    /// Invalidate the receipts root by setting it to a random value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_receipts_root: bool,

    /// Invalidate the gas used by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_gas_used: bool,

    /// Invalidate the block number by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_block_number: bool,

    /// Invalidate the timestamp by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_timestamp: bool,

    /// Invalidate the base fee by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_base_fee: bool,

    /// Invalidate the transactions by modifying them.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_transactions: bool,

    /// Invalidate the block hash by not recalculating it after modifications.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_block_hash: bool,

    /// Invalidate the withdrawals by modifying them.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_withdrawals: bool,

    /// Invalidate the blob gas used by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_blob_gas_used: bool,

    /// Invalidate the excess blob gas by setting it to an incorrect value.
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_excess_blob_gas: bool,

    /// Invalidate the requests hash by setting it to a random value (EIP-7685).
    #[arg(long, default_value_t = false, help_heading = "Auto-Invalidation Flags")]
    invalidate_requests_hash: bool,

    // ==================== Meta Flags ====================
    /// Skip block hash recalculation after modifications.
    #[arg(long, default_value_t = false, help_heading = "Meta Flags")]
    skip_hash_recalc: bool,

    /// Print what would be done without actually sending the payload.
    #[arg(long, default_value_t = false, help_heading = "Meta Flags")]
    dry_run: bool,
}

#[derive(Debug, Clone, clap::ValueEnum)]
enum Mode {
    /// Execute the `cast` command. This works with blocks of any size, because it pipes the
    /// payload into the `cast` command.
    Execute,
    /// Print the `cast` command. Caution: this may not work with large blocks because of the
    /// command length limit.
    Cast,
    /// Print the JSON payload. Can be piped into `cast` command if the block is small enough.
    Json,
}

impl Command {
    /// Build `InvalidationConfig` from command flags
    const fn build_invalidation_config(&self) -> InvalidationConfig {
        InvalidationConfig {
            parent_hash: self.parent_hash,
            fee_recipient: self.fee_recipient,
            state_root: self.state_root,
            receipts_root: self.receipts_root,
            logs_bloom: None,
            prev_randao: None,
            block_number: self.block_number,
            gas_limit: self.gas_limit,
            gas_used: self.gas_used,
            timestamp: self.timestamp,
            extra_data: None,
            base_fee_per_gas: self.base_fee_per_gas,
            block_hash: self.block_hash,
            blob_gas_used: self.blob_gas_used,
            excess_blob_gas: self.excess_blob_gas,
            invalidate_parent_hash: self.invalidate_parent_hash,
            invalidate_state_root: self.invalidate_state_root,
            invalidate_receipts_root: self.invalidate_receipts_root,
            invalidate_gas_used: self.invalidate_gas_used,
            invalidate_block_number: self.invalidate_block_number,
            invalidate_timestamp: self.invalidate_timestamp,
            invalidate_base_fee: self.invalidate_base_fee,
            invalidate_transactions: self.invalidate_transactions,
            invalidate_block_hash: self.invalidate_block_hash,
            invalidate_withdrawals: self.invalidate_withdrawals,
            invalidate_blob_gas_used: self.invalidate_blob_gas_used,
            invalidate_excess_blob_gas: self.invalidate_excess_blob_gas,
        }
    }

    /// Execute the command
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        let block_json = read_input(self.path.as_deref())?;
        let jwt_secret = load_jwt_secret(self.jwt_secret.as_deref())?;

        let block = serde_json::from_str::<AnyRpcBlock>(&block_json)?
            .into_inner()
            .map_header(|header| header.map(|h| h.into_header_with_defaults()))
            .try_map_transactions(|tx| tx.try_into_either::<OpTxEnvelope>())?
            .into_consensus();

        let config = self.build_invalidation_config();

        let parent_beacon_block_root =
            self.parent_beacon_block_root.or(block.header.parent_beacon_block_root);
        let blob_versioned_hashes =
            block.body.blob_versioned_hashes_iter().copied().collect::<Vec<_>>();
        let use_v4 = block.header.requests_hash.is_some();
        let requests_hash = self.requests_hash.or(block.header.requests_hash);

        let mut execution_payload = ExecutionPayload::from_block_slow(&block).0;

        let changes = match &mut execution_payload {
            ExecutionPayload::V1(p) => config.apply_to_payload_v1(p),
            ExecutionPayload::V2(p) => config.apply_to_payload_v2(p),
            ExecutionPayload::V3(p) => config.apply_to_payload_v3(p),
            ExecutionPayload::V4(p) => config.apply_to_payload_v3(&mut p.payload_inner),
        };

        let skip_recalc = self.skip_hash_recalc || config.should_skip_hash_recalc();
        if !skip_recalc {
            let new_hash = match execution_payload.clone().into_block_raw() {
                Ok(block) => block.header.hash_slow(),
                Err(e) => {
                    eprintln!(
                        "Warning: Could not recalculate block hash: {e}. Using original hash."
                    );
                    match &execution_payload {
                        ExecutionPayload::V1(p) => p.block_hash,
                        ExecutionPayload::V2(p) => p.payload_inner.block_hash,
                        ExecutionPayload::V3(p) => p.payload_inner.payload_inner.block_hash,
                        ExecutionPayload::V4(p) => {
                            p.payload_inner.payload_inner.payload_inner.block_hash
                        }
                    }
                }
            };

            match &mut execution_payload {
                ExecutionPayload::V1(p) => p.block_hash = new_hash,
                ExecutionPayload::V2(p) => p.payload_inner.block_hash = new_hash,
                ExecutionPayload::V3(p) => p.payload_inner.payload_inner.block_hash = new_hash,
                ExecutionPayload::V4(p) => {
                    p.payload_inner.payload_inner.payload_inner.block_hash = new_hash
                }
            }
        }

        if self.dry_run {
            println!("=== Dry Run ===");
            println!("Changes that would be applied:");
            for change in &changes {
                println!("  - {}", change);
            }
            if changes.is_empty() {
                println!("  (no changes)");
            }
            if skip_recalc {
                println!("  - Block hash recalculation: SKIPPED");
            } else {
                println!("  - Block hash recalculation: PERFORMED");
            }
            println!("\nResulting payload JSON:");
            let json = serde_json::to_string_pretty(&execution_payload)?;
            println!("{}", json);
            return Ok(());
        }

        let json_request = if use_v4 {
            serde_json::to_string(&(
                execution_payload,
                blob_versioned_hashes,
                parent_beacon_block_root,
                requests_hash.unwrap_or_default(),
            ))?
        } else {
            serde_json::to_string(&(
                execution_payload,
                blob_versioned_hashes,
                parent_beacon_block_root,
            ))?
        };

        match self.mode {
            Mode::Execute => {
                let mut command = std::process::Command::new("cast");
                let method = if use_v4 { "engine_newPayloadV4" } else { "engine_newPayloadV3" };
                command.arg("rpc").arg(method).arg("--raw");
                if let Some(rpc_url) = self.rpc_url {
                    command.arg("--rpc-url").arg(rpc_url);
                }
                if let Some(secret) = &jwt_secret {
                    command.arg("--jwt-secret").arg(secret);
                }

                let mut process = command.stdin(std::process::Stdio::piped()).spawn()?;

                process
                    .stdin
                    .take()
                    .ok_or_eyre("stdin not available")?
                    .write_all(json_request.as_bytes())?;

                process.wait()?;
            }
            Mode::Cast => {
                let mut cmd = format!(
                    "cast rpc engine_newPayloadV{} --raw '{}'",
                    self.new_payload_version, json_request
                );

                if let Some(rpc_url) = self.rpc_url {
                    cmd += &format!(" --rpc-url {rpc_url}");
                }
                if let Some(secret) = &jwt_secret {
                    cmd += &format!(" --jwt-secret {secret}");
                }

                println!("{cmd}");
            }
            Mode::Json => {
                println!("{json_request}");
            }
        }

        Ok(())
    }
}
