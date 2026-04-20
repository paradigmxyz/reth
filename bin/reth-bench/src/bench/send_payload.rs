use super::helpers::{fetch_block_access_list, load_jwt_secret, read_input};
use alloy_consensus::TxEnvelope;
use alloy_primitives::Bytes;
use alloy_provider::{
    network::{AnyNetwork, AnyRpcBlock},
    RootProvider,
};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::ExecutionPayload;
use clap::Parser;
use eyre::{OptionExt, Result};
use reth_cli_runner::CliContext;
use std::io::Write;

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
    /// Execute the generate payload command
    pub async fn execute(self, _ctx: CliContext) -> Result<()> {
        // Load block
        let block_json = read_input(self.path.as_deref())?;

        // Load JWT secret
        let jwt_secret = load_jwt_secret(self.jwt_secret.as_deref())?;

        // Parse the block
        let block = serde_json::from_str::<AnyRpcBlock>(&block_json)?
            .into_inner()
            .map_header(|header| header.map(|h| h.into_header_with_defaults()))
            .try_map_transactions(|tx| -> eyre::Result<TxEnvelope> {
                tx.try_into().map_err(|_| eyre::eyre!("unsupported tx type"))
            })?
            .into_consensus();

        let use_v4 = block.header.requests_hash.is_some();
        let use_v5 = block.header.block_access_list_hash.is_some();

        // Extract parent beacon block root
        let parent_beacon_block_root = block.header.parent_beacon_block_root;

        // Extract blob versioned hashes
        let blob_versioned_hashes =
            block.body.blob_versioned_hashes_iter().copied().collect::<Vec<_>>();

        // V5 payloads must carry the full RLP-encoded block access list, not just the hash stored
        // in the header.
        let execution_payload = if use_v5 {
            let encoded_bal = self.fetch_encoded_block_access_list(block.header.number).await?;
            ExecutionPayload::from_block_slow_with_bal(&block, encoded_bal).0
        } else {
            ExecutionPayload::from_block_slow(&block).0
        };

        // Create JSON request data
        let json_request = if use_v4 {
            serde_json::to_string(&(
                execution_payload,
                blob_versioned_hashes,
                parent_beacon_block_root,
                block.header.requests_hash.unwrap_or_default(),
            ))?
        } else {
            serde_json::to_string(&(
                execution_payload,
                blob_versioned_hashes,
                parent_beacon_block_root,
            ))?
        };

        // Print output or execute command
        match self.mode {
            Mode::Execute => {
                // Create cast command
                let mut command = std::process::Command::new("cast");
                let method = if use_v5 {
                    "engine_newPayloadV5"
                } else if use_v4 {
                    "engine_newPayloadV4"
                } else {
                    "engine_newPayloadV3"
                };
                command.arg("rpc").arg(method).arg("--raw");
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

    async fn fetch_encoded_block_access_list(&self, block_number: u64) -> Result<Bytes> {
        let rpc_url = self
            .rpc_url
            .as_deref()
            .ok_or_eyre("--rpc-url is required to fetch the block access list for V5 payloads")?;
        let client = ClientBuilder::default()
            .layer(alloy_transport::layers::RetryBackoffLayer::new(10, 800, u64::MAX))
            .http(rpc_url.parse()?);
        let provider = RootProvider::<AnyNetwork>::new(client);
        let bal = fetch_block_access_list(&provider, block_number).await?;
        Ok(alloy_rlp::encode(bal).into())
    }
}
