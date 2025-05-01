use alloy_provider::network::AnyRpcBlock;
use alloy_rpc_types_engine::ExecutionPayload;
use clap::Parser;
use eyre::{OptionExt, Result};
use op_alloy_consensus::OpTxEnvelope;
use reth_cli_runner::CliContext;
use std::io::{BufReader, Read, Write};

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
            None => String::from_utf8(
                BufReader::new(std::io::stdin()).bytes().collect::<Result<Vec<_>, _>>()?,
            )?,
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
        let block = serde_json::from_str::<AnyRpcBlock>(&block_json)?
            .into_inner()
            .map_header(|header| header.map(|h| h.into_header_with_defaults()))
            .try_map_transactions(|tx| {
                // try to convert unknowns into op type so that we can also support optimism
                tx.try_into_either::<OpTxEnvelope>()
            })?
            .into_consensus();

        // Extract parent beacon block root
        let parent_beacon_block_root = block.header.parent_beacon_block_root;

        // Extract blob versioned hashes
        let blob_versioned_hashes =
            block.body.blob_versioned_hashes_iter().copied().collect::<Vec<_>>();

        // Convert to execution payload
        let execution_payload = ExecutionPayload::from_block_slow(&block).0;

        let use_v4 = block.header.requests_hash.is_some();

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
                let method = if use_v4 { "engine_newPayloadV4" } else { "engine_newPayloadV3" };
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
}
