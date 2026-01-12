//! Command for replaying pre-generated payloads from disk.
//!
//! This command reads `ExecutionPayloadEnvelopeV4` files from a directory and replays them
//! in sequence using `newPayload` followed by `forkchoiceUpdated`.

use crate::{
    authenticated_transport::AuthenticatedTransportConnect,
    valid_payload::{call_forkchoice_updated, call_new_payload},
};
use alloy_primitives::B256;
use alloy_provider::{ext::EngineApi, network::AnyNetwork, Provider, RootProvider};
use alloy_rpc_client::ClientBuilder;
use alloy_rpc_types_engine::{ExecutionPayloadEnvelopeV4, ForkchoiceState, JwtSecret};
use clap::Parser;
use eyre::Context;
use reth_node_api::EngineApiMessageVersion;
use reqwest::Url;
use reth_cli_runner::CliContext;
use std::path::PathBuf;
use tracing::{debug, info};

/// `reth bench replay-payloads` command
///
/// Replays pre-generated payloads from a directory by calling `newPayload` followed by
/// `forkchoiceUpdated` for each payload in sequence.
#[derive(Debug, Parser)]
pub struct Command {
    /// The engine RPC URL (with JWT authentication).
    #[arg(long, value_name = "ENGINE_RPC_URL", default_value = "http://localhost:8551")]
    engine_rpc_url: String,

    /// Path to the JWT secret file for engine API authentication.
    #[arg(long, value_name = "JWT_SECRET")]
    jwt_secret: PathBuf,

    /// Directory containing payload files (payload_001.json, payload_002.json, etc.).
    #[arg(long, value_name = "PAYLOAD_DIR")]
    payload_dir: PathBuf,

    /// Optional limit on the number of payloads to replay.
    /// If not specified, replays all payloads in the directory.
    #[arg(long, value_name = "COUNT")]
    count: Option<usize>,

    /// Skip the first N payloads.
    #[arg(long, value_name = "SKIP", default_value = "0")]
    skip: usize,

    /// Optional directory containing gas ramp payloads to replay first.
    /// These are replayed before the main payloads to warm up the gas limit.
    #[arg(long, value_name = "GAS_RAMP_DIR")]
    gas_ramp_dir: Option<PathBuf>,
}

/// A loaded payload ready for execution.
struct LoadedPayload {
    /// The index (from filename).
    index: u64,
    /// The payload envelope.
    envelope: ExecutionPayloadEnvelopeV4,
    /// The block hash.
    block_hash: B256,
}

/// A gas ramp payload loaded from disk.
struct GasRampPayload {
    /// Block number from filename.
    block_number: u64,
    /// Engine API version for newPayload.
    version: EngineApiMessageVersion,
    /// The block hash for FCU.
    block_hash: B256,
    /// The params to pass to newPayload.
    params: serde_json::Value,
}

impl Command {
    /// Execute the `replay-payloads` command.
    pub async fn execute(self, _ctx: CliContext) -> eyre::Result<()> {
        info!(payload_dir = %self.payload_dir.display(), "Replaying payloads");

        // Set up authenticated engine provider
        let jwt =
            std::fs::read_to_string(&self.jwt_secret).wrap_err("Failed to read JWT secret file")?;
        let jwt = JwtSecret::from_hex(jwt.trim())?;
        let auth_url = Url::parse(&self.engine_rpc_url)?;

        info!("Connecting to Engine RPC at {}", auth_url);
        let auth_transport = AuthenticatedTransportConnect::new(auth_url.clone(), jwt);
        let auth_client = ClientBuilder::default().connect_with(auth_transport).await?;
        let auth_provider = RootProvider::<AnyNetwork>::new(auth_client);

        // Get parent block (latest canonical block) - we need this for the first FCU
        let parent_block = auth_provider
            .get_block_by_number(alloy_eips::BlockNumberOrTag::Latest)
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to fetch latest block"))?;

        let initial_parent_hash = parent_block.header.hash;
        let initial_parent_number = parent_block.header.number;

        info!(
            parent_hash = %initial_parent_hash,
            parent_number = initial_parent_number,
            "Using initial parent block"
        );

        // Load all payloads upfront to avoid I/O delays between phases
        let gas_ramp_payloads = if let Some(ref gas_ramp_dir) = self.gas_ramp_dir {
            let payloads = self.load_gas_ramp_payloads(gas_ramp_dir)?;
            if payloads.is_empty() {
                return Err(eyre::eyre!("No gas ramp payload files found in {:?}", gas_ramp_dir));
            }
            info!(count = payloads.len(), "Loaded gas ramp payloads from disk");
            payloads
        } else {
            Vec::new()
        };

        let payloads = self.load_payloads()?;
        if payloads.is_empty() {
            return Err(eyre::eyre!("No payload files found in {:?}", self.payload_dir));
        }
        info!(count = payloads.len(), "Loaded main payloads from disk");

        let mut parent_hash = initial_parent_hash;

        // Replay gas ramp payloads first
        for (i, payload) in gas_ramp_payloads.iter().enumerate() {
            info!(
                gas_ramp_payload = i + 1,
                total = gas_ramp_payloads.len(),
                block_number = payload.block_number,
                block_hash = %payload.block_hash,
                "Executing gas ramp payload (newPayload + FCU)"
            );

            call_new_payload(&auth_provider, payload.version, payload.params.clone()).await?;

            let fcu_state = ForkchoiceState {
                head_block_hash: payload.block_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };
            call_forkchoice_updated(&auth_provider, payload.version, fcu_state, None).await?;

            info!(gas_ramp_payload = i + 1, "Gas ramp payload executed successfully");
            parent_hash = payload.block_hash;
        }

        if !gas_ramp_payloads.is_empty() {
            info!(count = gas_ramp_payloads.len(), "All gas ramp payloads replayed");
        }

        for (i, payload) in payloads.iter().enumerate() {
            info!(
                payload = i + 1,
                total = payloads.len(),
                index = payload.index,
                block_hash = %payload.block_hash,
                "Executing payload (newPayload + FCU)"
            );

            self.execute_payload_v4(&auth_provider, &payload.envelope, parent_hash).await?;

            info!(payload = i + 1, "Payload executed successfully");
            parent_hash = payload.block_hash;
        }

        info!(count = payloads.len(), "All payloads replayed successfully");
        Ok(())
    }

    /// Load and parse all payload files from the directory.
    fn load_payloads(&self) -> eyre::Result<Vec<LoadedPayload>> {
        let mut payloads = Vec::new();

        // Read directory entries
        let entries: Vec<_> = std::fs::read_dir(&self.payload_dir)
            .wrap_err_with(|| format!("Failed to read directory {:?}", self.payload_dir))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("json") &&
                    e.file_name().to_string_lossy().starts_with("payload_")
            })
            .collect();

        // Parse filenames to get indices and sort
        let mut indexed_paths: Vec<(u64, PathBuf)> = entries
            .into_iter()
            .filter_map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                // Extract index from "payload_NNN.json"
                let index_str = name_str.strip_prefix("payload_")?.strip_suffix(".json")?;
                let index: u64 = index_str.parse().ok()?;
                Some((index, e.path()))
            })
            .collect();

        indexed_paths.sort_by_key(|(idx, _)| *idx);

        // Apply skip and count
        let indexed_paths: Vec<_> = indexed_paths.into_iter().skip(self.skip).collect();
        let indexed_paths: Vec<_> = match self.count {
            Some(count) => indexed_paths.into_iter().take(count).collect(),
            None => indexed_paths,
        };

        // Load each payload
        for (index, path) in indexed_paths {
            let content = std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("Failed to read {:?}", path))?;
            let envelope: ExecutionPayloadEnvelopeV4 = serde_json::from_str(&content)
                .wrap_err_with(|| format!("Failed to parse {:?}", path))?;

            let block_hash =
                envelope.envelope_inner.execution_payload.payload_inner.payload_inner.block_hash;

            info!(
                index = index,
                block_hash = %block_hash,
                path = %path.display(),
                "Loaded payload"
            );

            payloads.push(LoadedPayload { index, envelope, block_hash });
        }

        Ok(payloads)
    }

    /// Load and parse gas ramp payload files from a directory.
    fn load_gas_ramp_payloads(&self, dir: &PathBuf) -> eyre::Result<Vec<GasRampPayload>> {
        let mut payloads = Vec::new();

        let entries: Vec<_> = std::fs::read_dir(dir)
            .wrap_err_with(|| format!("Failed to read directory {:?}", dir))?
            .filter_map(|e| e.ok())
            .filter(|e| {
                e.path().extension().and_then(|s| s.to_str()) == Some("json") &&
                    e.file_name().to_string_lossy().starts_with("payload_block_")
            })
            .collect();

        // Parse filenames to get block numbers and sort
        let mut indexed_paths: Vec<(u64, PathBuf)> = entries
            .into_iter()
            .filter_map(|e| {
                let name = e.file_name();
                let name_str = name.to_string_lossy();
                // Extract block number from "payload_block_NNN.json"
                let block_str = name_str.strip_prefix("payload_block_")?.strip_suffix(".json")?;
                let block_number: u64 = block_str.parse().ok()?;
                Some((block_number, e.path()))
            })
            .collect();

        indexed_paths.sort_by_key(|(num, _)| *num);

        for (block_number, path) in indexed_paths {
            let content = std::fs::read_to_string(&path)
                .wrap_err_with(|| format!("Failed to read {:?}", path))?;
            let wrapper: serde_json::Value = serde_json::from_str(&content)
                .wrap_err_with(|| format!("Failed to parse {:?}", path))?;

            let version_num = wrapper["version"]
                .as_u64()
                .ok_or_else(|| eyre::eyre!("Missing version in {:?}", path))?;
            let version = match version_num {
                1 => EngineApiMessageVersion::V1,
                2 => EngineApiMessageVersion::V2,
                3 => EngineApiMessageVersion::V3,
                4 => EngineApiMessageVersion::V4,
                5 => EngineApiMessageVersion::V5,
                _ => return Err(eyre::eyre!("Invalid version {} in {:?}", version_num, path)),
            };

            let block_hash_str = wrapper["block_hash"]
                .as_str()
                .ok_or_else(|| eyre::eyre!("Missing block_hash in {:?}", path))?;
            let block_hash: B256 = block_hash_str
                .parse()
                .map_err(|_| eyre::eyre!("Invalid block_hash in {:?}", path))?;

            let params = wrapper["params"].clone();

            info!(
                block_number = block_number,
                block_hash = %block_hash,
                path = %path.display(),
                "Loaded gas ramp payload"
            );

            payloads.push(GasRampPayload { block_number, version, block_hash, params });
        }

        Ok(payloads)
    }

    async fn execute_payload_v4(
        &self,
        provider: &RootProvider<AnyNetwork>,
        envelope: &ExecutionPayloadEnvelopeV4,
        parent_hash: B256,
    ) -> eyre::Result<()> {
        let block_hash =
            envelope.envelope_inner.execution_payload.payload_inner.payload_inner.block_hash;

        debug!(
            method = "engine_newPayloadV4",
            block_hash = %block_hash,
            "Sending newPayload"
        );

        let status = provider
            .new_payload_v4(
                envelope.envelope_inner.execution_payload.clone(),
                vec![],
                B256::ZERO,
                envelope.execution_requests.to_vec(),
            )
            .await?;

        info!(?status, "newPayloadV4 response");

        if !status.is_valid() {
            return Err(eyre::eyre!("Payload rejected: {:?}", status));
        }

        let fcu_state = ForkchoiceState {
            head_block_hash: block_hash,
            safe_block_hash: parent_hash,
            finalized_block_hash: parent_hash,
        };

        debug!(method = "engine_forkchoiceUpdatedV3", ?fcu_state, "Sending forkchoiceUpdated");

        let fcu_result = provider.fork_choice_updated_v3(fcu_state, None).await?;

        info!(?fcu_result, "forkchoiceUpdatedV3 response");

        Ok(())
    }
}
