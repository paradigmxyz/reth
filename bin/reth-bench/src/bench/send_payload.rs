use alloy_consensus::{
    Block as PrimitiveBlock, BlockBody, Header as PrimitiveHeader,
    Transaction as PrimitiveTransaction,
};
use alloy_rpc_types::{Block as RpcBlock, BlockTransactions};
use alloy_rpc_types_engine::ExecutionPayload;
use clap::Parser;
use eyre::{OptionExt, Result};
use reth_cli_runner::CliContext;
use std::io::{Read, Write};

use super::rpc_transaction::RpcTransaction;

/// Command for generating and sending an `engine_newPayload` request constructed from an RPC
/// block.
#[derive(Debug, Parser)]
pub struct Command {
    /// Path to the json file to parse. If not specified, stdin will be used.
    #[arg(short, long)]
    path: Option<String>,

    /// The engine RPC url to use.
    #[arg(
        short,
        long,
        // Required if `mode` is `execute` or `cast`.
        required_if_eq_any([("mode", "execute"), ("mode", "cast")]),
        // If `mode` is not specified, then `execute` is used, so we need to require it.
        required_unless_present("mode")
    )]
    rpc_url: Option<String>,

    /// The JWT secret to use. Can be either a path to a file containing the secret or the secret
    /// itself.
    #[arg(short, long)]
    jwt_secret: Option<String>,

    #[arg(long, default_value_t = 3)]
    new_payload_version: u8,

    /// The mode to use.
    #[arg(long, value_enum, default_value = "execute")]
    mode: Mode,
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
    /// Read input from either a file or stdin
    fn read_input(&self) -> Result<String> {
        Ok(match &self.path {
            Some(path) => reth_fs_util::read_to_string(path)?,
            None => String::from_utf8(std::io::stdin().bytes().collect::<Result<Vec<_>, _>>()?)?,
        })
    }

    /// Load JWT secret from either a file or use the provided string directly
    fn load_jwt_secret(&self) -> Result<Option<String>> {
        match &self.jwt_secret {
            Some(secret) => {
                // Try to read as file first
                match std::fs::read_to_string(secret) {
                    Ok(contents) => Ok(Some(contents.trim().to_string())),
                    // If file read fails, use the string directly
                    Err(_) => Ok(Some(secret.clone())),
                }
            }
            None => Ok(None),
        }
    }

    /// Execute the generate payload command
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        // Load block
        let block_json = self.read_input()?;

        // Load JWT secret
        let jwt_secret = self.load_jwt_secret()?;

        // Parse the block
        let block: RpcBlock<RpcTransaction> = serde_json::from_str(&block_json)?;

        // Extract parent beacon block root
        let parent_beacon_block_root = block.header.parent_beacon_block_root;

        // Extract transactions
        let transactions = match block.transactions {
            BlockTransactions::Hashes(_) => {
                return Err(eyre::eyre!("Block must include full transaction data. Send the eth_getBlockByHash request with full: `true`"));
            }
            BlockTransactions::Full(txs) => txs,
            BlockTransactions::Uncle => {
                return Err(eyre::eyre!("Cannot process uncle blocks"));
            }
        };

        // Extract blob versioned hashes
        let blob_versioned_hashes = transactions
            .iter()
            .filter_map(|tx| tx.blob_versioned_hashes().map(|v| v.to_vec()))
            .flatten()
            .collect::<Vec<_>>();

        // Convert to execution payload
        let execution_payload = ExecutionPayload::from_block_slow(&PrimitiveBlock::new(
            PrimitiveHeader::from(block.header),
            BlockBody { transactions, ommers: vec![], withdrawals: block.withdrawals },
        ))
        .0;

        // Create JSON request data
        let json_request = serde_json::to_string(&(
            execution_payload,
            blob_versioned_hashes,
            parent_beacon_block_root,
        ))?;

        // Print output or execute command
        match self.mode {
            Mode::Execute => {
                // Create cast command
                let mut command = std::process::Command::new("cast");
                command.arg("rpc").arg("engine_newPayloadV3").arg("--raw");
                if let Some(rpc_url) = self.rpc_url {
                    command.arg("--rpc-url").arg(rpc_url);
                }
                if let Some(secret) = &jwt_secret {
                    command.arg("--jwt-secret").arg(secret);
                }

                // Start cast process
                let mut process = command.stdin(std::process::Stdio::piped()).spawn()?;

                // Write to cast's stdin
                process
                    .stdin
                    .take()
                    .ok_or_eyre("stdin not available")?
                    .write_all(json_request.as_bytes())?;

                // Wait for cast to finish
                process.wait()?;
            }
            Mode::Cast => {
                let mut cmd = format!(
                    "cast rpc engine_newPayloadV{} --raw '{}'",
                    self.new_payload_version, json_request
                );

                if let Some(rpc_url) = self.rpc_url {
                    cmd += &format!(" --rpc-url {}", rpc_url);
                }
                if let Some(secret) = &jwt_secret {
                    cmd += &format!(" --jwt-secret {}", secret);
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
