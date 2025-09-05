//! Engine API specific actions for testing.

use crate::testsuite::{Action, Environment};
use alloy_rpc_types_engine::{
    ExecutionPayloadV1, ExecutionPayloadV2, ExecutionPayloadV3, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::marker::PhantomData;
use tracing::debug;

// Constants
const MAX_BLOCK_RETRIEVAL_RETRIES: u32 = 5;
const BLOCK_RETRIEVAL_RETRY_DELAY_MS: u64 = 100;

/// Represents different payload data sources
#[derive(Debug, Clone)]
pub enum PayloadSource {
    /// Load payload from embedded JSON string
    Json(String),
    /// Load payload from file path
    File(String),
    /// Load payload from another node by block number
    Node { 
        /// Index of the source node to load from
        source_node_idx: usize, 
        /// Block number to retrieve
        block_number: u64 
    },
}

/// Represents different JSON payload formats
#[derive(Debug)]
enum PayloadJsonFormat {
    /// Array format: [block, `versioned_hashes`, `parent_beacon_block_root`, `execution_requests_hash`]
    Array { block: serde_json::Value, versioned_hashes: Vec<alloy_primitives::B256> },
    /// Object format: just the block object
    Object(serde_json::Value),
}

/// Common payload sending configuration
#[derive(Debug, Clone)]
pub struct PayloadConfig {
    /// Node index to send to
    pub node_idx: Option<usize>,
    /// Expected payload status
    pub expected_status: ExpectedPayloadStatus,
}

impl Default for PayloadConfig {
    fn default() -> Self {
        Self { node_idx: None, expected_status: ExpectedPayloadStatus::Valid }
    }
}

/// Action that sends a newPayload request to a specific node.
#[derive(Debug)]
pub struct SendNewPayload<Engine>
where
    Engine: EngineTypes,
{
    /// The node index to send to
    pub node_idx: usize,
    /// The block number to send
    pub block_number: u64,
    /// The source node to get the block from
    pub source_node_idx: usize,
    /// Expected payload status
    pub expected_status: ExpectedPayloadStatus,
    _phantom: PhantomData<Engine>,
}

/// Expected status for a payload
#[derive(Debug, Clone)]
pub enum ExpectedPayloadStatus {
    /// Expect the payload to be valid
    Valid,
    /// Expect the payload to be invalid
    Invalid,
    /// Expect the payload to be syncing or accepted (buffered)
    SyncingOrAccepted,
}

impl<Engine> SendNewPayload<Engine>
where
    Engine: EngineTypes,
{
    /// Create a new `SendNewPayload` action
    pub fn new(
        node_idx: usize,
        block_number: u64,
        source_node_idx: usize,
        expected_status: ExpectedPayloadStatus,
    ) -> Self {
        Self {
            node_idx,
            block_number,
            source_node_idx,
            expected_status,
            _phantom: Default::default(),
        }
    }
}

impl<Engine> Action<Engine> for SendNewPayload<Engine>
where
    Engine: EngineTypes + PayloadTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Validate node indices
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Target node index out of bounds: {}", self.node_idx));
            }
            if self.source_node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!(
                    "Source node index out of bounds: {}",
                    self.source_node_idx
                ));
            }

            // Use the unified payload sender with node source
            let mut unified_sender =
                SendPayload::from_node(self.source_node_idx, self.block_number)
                    .with_node_idx(self.node_idx)
                    .with_expected_status(self.expected_status.clone());

            unified_sender.execute(env).await
        })
    }
}

/// Action that sends multiple blocks to a node in a specific order.
#[derive(Debug)]
pub struct SendNewPayloads<Engine>
where
    Engine: EngineTypes,
{
    /// The node index to send to
    target_node: Option<usize>,
    /// The source node to get the blocks from
    source_node: Option<usize>,
    /// The starting block number
    start_block: Option<u64>,
    /// The total number of blocks to send
    total_blocks: Option<u64>,
    /// Whether to send in reverse order
    reverse_order: bool,
    /// Custom block numbers to send (if not using `start_block` + `total_blocks`)
    custom_block_numbers: Option<Vec<u64>>,
    _phantom: PhantomData<Engine>,
}

impl<Engine> SendNewPayloads<Engine>
where
    Engine: EngineTypes,
{
    /// Create a new `SendNewPayloads` action builder
    pub fn new() -> Self {
        Self {
            target_node: None,
            source_node: None,
            start_block: None,
            total_blocks: None,
            reverse_order: false,
            custom_block_numbers: None,
            _phantom: Default::default(),
        }
    }

    /// Set the target node index
    pub const fn with_target_node(mut self, node_idx: usize) -> Self {
        self.target_node = Some(node_idx);
        self
    }

    /// Set the source node index
    pub const fn with_source_node(mut self, node_idx: usize) -> Self {
        self.source_node = Some(node_idx);
        self
    }

    /// Set the starting block number
    pub const fn with_start_block(mut self, block_num: u64) -> Self {
        self.start_block = Some(block_num);
        self
    }

    /// Set the total number of blocks to send
    pub const fn with_total_blocks(mut self, count: u64) -> Self {
        self.total_blocks = Some(count);
        self
    }

    /// Send blocks in reverse order (useful for testing buffering)
    pub const fn in_reverse_order(mut self) -> Self {
        self.reverse_order = true;
        self
    }

    /// Set custom block numbers to send
    pub fn with_block_numbers(mut self, block_numbers: Vec<u64>) -> Self {
        self.custom_block_numbers = Some(block_numbers);
        self
    }
}

impl<Engine> Default for SendNewPayloads<Engine>
where
    Engine: EngineTypes,
{
    fn default() -> Self {
        Self::new()
    }
}

impl<Engine> Action<Engine> for SendNewPayloads<Engine>
where
    Engine: EngineTypes + PayloadTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Validate required fields
            let target_node =
                self.target_node.ok_or_else(|| eyre::eyre!("Target node not specified"))?;
            let source_node =
                self.source_node.ok_or_else(|| eyre::eyre!("Source node not specified"))?;

            // Determine block numbers to send
            let block_numbers = if let Some(custom_numbers) = &self.custom_block_numbers {
                custom_numbers.clone()
            } else {
                let start =
                    self.start_block.ok_or_else(|| eyre::eyre!("Start block not specified"))?;
                let count =
                    self.total_blocks.ok_or_else(|| eyre::eyre!("Total blocks not specified"))?;

                if self.reverse_order {
                    // Send blocks in reverse order (e.g., for count=2, start=1: [2, 1])
                    (0..count).map(|i| start + count - 1 - i).collect()
                } else {
                    // Send blocks in normal order
                    (0..count).map(|i| start + i).collect()
                }
            };

            for &block_number in &block_numbers {
                // For the first block in reverse order, expect buffering
                // For subsequent blocks, they might connect immediately
                let expected_status =
                    if self.reverse_order && block_number == *block_numbers.first().unwrap() {
                        ExpectedPayloadStatus::SyncingOrAccepted
                    } else {
                        ExpectedPayloadStatus::Valid
                    };

                let mut action = SendNewPayload::<Engine>::new(
                    target_node,
                    block_number,
                    source_node,
                    expected_status,
                );

                action.execute(env).await?;
            }

            Ok(())
        })
    }
}

/// Helper function to convert a block to `ExecutionPayloadV3`
fn block_to_payload_v3(block: Block) -> ExecutionPayloadV3 {
    use alloy_primitives::U256;

    ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash: block.header.inner.parent_hash,
                fee_recipient: block.header.inner.beneficiary,
                state_root: block.header.inner.state_root,
                receipts_root: block.header.inner.receipts_root,
                logs_bloom: block.header.inner.logs_bloom,
                prev_randao: block.header.inner.mix_hash,
                block_number: block.header.inner.number,
                gas_limit: block.header.inner.gas_limit,
                gas_used: block.header.inner.gas_used,
                timestamp: block.header.inner.timestamp,
                extra_data: block.header.inner.extra_data.clone(),
                base_fee_per_gas: U256::from(block.header.inner.base_fee_per_gas.unwrap_or(0)),
                block_hash: block.header.hash,
                transactions: vec![], // No transactions needed for buffering tests
            },
            withdrawals: block.withdrawals.unwrap_or_default().to_vec(),
        },
        blob_gas_used: block.header.inner.blob_gas_used.unwrap_or(0),
        excess_blob_gas: block.header.inner.excess_blob_gas.unwrap_or(0),
    }
}

/// Parse JSON payload data into a structured format
fn parse_payload_json(json_data: &str) -> Result<PayloadJsonFormat> {
    let json_value: serde_json::Value = serde_json::from_str(json_data)
        .map_err(|e| eyre::eyre!("Failed to parse payload JSON: {}", e))?;

    if json_value.is_array() {
        // Array format: [block, versioned_hashes, parent_beacon_block_root,
        // execution_requests_hash]
        let payload_array = json_value.as_array().unwrap();
        if payload_array.len() < 2 {
            return Err(eyre::eyre!(
                "Payload array must contain at least block data and versioned hashes"
            ));
        }

        let block = payload_array[0].clone();
        let versioned_hashes_json = &payload_array[1];
        let versioned_hashes: Vec<alloy_primitives::B256> =
            serde_json::from_value(versioned_hashes_json.clone())
                .map_err(|e| eyre::eyre!("Failed to parse versioned hashes: {}", e))?;

        Ok(PayloadJsonFormat::Array { block, versioned_hashes })
    } else {
        // Object format: just the block object
        Ok(PayloadJsonFormat::Object(json_value))
    }
}

/// Load payload data from the specified source
async fn load_payload_data(source: &PayloadSource) -> Result<String> {
    match source {
        PayloadSource::Json(json_data) => Ok(json_data.clone()),
        PayloadSource::File(file_path) => {
            // Handle path resolution for relative paths
            let resolved_path = if file_path.starts_with("crates/") {
                // Try multiple possible locations
                let candidates = vec![
                    std::path::Path::new(file_path).to_path_buf(),
                    std::path::Path::new("../../..").join(file_path),
                    std::path::Path::new("../../../..").join(file_path),
                ];

                let mut found_path = None;
                for candidate in candidates {
                    if candidate.exists() {
                        found_path = Some(candidate);
                        break;
                    }
                }

                found_path.ok_or_else(|| {
                    eyre::eyre!(
                        "Could not find payload file {} in any expected location",
                        file_path
                    )
                })?
            } else {
                std::path::Path::new(file_path).to_path_buf()
            };

            std::fs::read_to_string(&resolved_path).map_err(|e| {
                eyre::eyre!("Failed to read payload file {}: {}", resolved_path.display(), e)
            })
        }
        PayloadSource::Node { .. } => {
            Err(eyre::eyre!("Node-based payload source should be handled separately"))
        }
    }
}

/// Validate payload response against expected status
fn validate_payload_response(
    result: &alloy_rpc_types_engine::PayloadStatus,
    expected: &ExpectedPayloadStatus,
    node_idx: usize,
    context: &str,
) -> Result<()> {
    match (&result.status, expected) {
        (PayloadStatusEnum::Valid, ExpectedPayloadStatus::Valid) => {
            debug!("Node {}: {} marked as VALID as expected", node_idx, context);
            Ok(())
        }
        (PayloadStatusEnum::Invalid { validation_error }, ExpectedPayloadStatus::Invalid) => {
            debug!(
                "Node {}: {} marked as INVALID as expected: {:?}",
                node_idx, context, validation_error
            );
            Ok(())
        }
        (
            PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted,
            ExpectedPayloadStatus::SyncingOrAccepted,
        ) => {
            debug!("Node {}: {} marked as SYNCING/ACCEPTED as expected", node_idx, context);
            Ok(())
        }
        (status, expected) => {
            let error_msg = if let Some(ref latest_valid_hash) = result.latest_valid_hash {
                format!(
                    "Node {}: Unexpected payload status for {}. Expected: {:?}, Got: {:?}, Latest valid hash: {:?}",
                    node_idx, context, expected, status, latest_valid_hash
                )
            } else {
                format!(
                    "Node {}: Unexpected payload status for {}. Expected: {:?}, Got: {:?}",
                    node_idx, context, expected, status
                )
            };
            Err(eyre::eyre!(error_msg))
        }
    }
}

/// Unified action to send a payload from various sources (JSON, file, or node)
#[derive(Debug)]
pub struct SendPayload<Engine> {
    /// Payload data source
    pub source: PayloadSource,
    /// Payload configuration
    pub config: PayloadConfig,
    _phantom: PhantomData<Engine>,
}

impl<Engine> SendPayload<Engine> {
    /// Create a new `SendPayload` action from JSON data
    pub fn from_json(json_data: impl Into<String>) -> Self {
        Self {
            source: PayloadSource::Json(json_data.into()),
            config: PayloadConfig::default(),
            _phantom: Default::default(),
        }
    }

    /// Create a new `SendPayload` action from file
    pub fn from_file(file_path: impl Into<String>) -> Self {
        Self {
            source: PayloadSource::File(file_path.into()),
            config: PayloadConfig::default(),
            _phantom: Default::default(),
        }
    }

    /// Create a new `SendPayload` action from another node
    pub fn from_node(source_node_idx: usize, block_number: u64) -> Self {
        Self {
            source: PayloadSource::Node { source_node_idx, block_number },
            config: PayloadConfig::default(),
            _phantom: Default::default(),
        }
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.config.node_idx = Some(idx);
        self
    }

    /// Set expected status for the payload
    pub const fn with_expected_status(mut self, status: ExpectedPayloadStatus) -> Self {
        self.config.expected_status = status;
        self
    }
}

impl<Engine> Action<Engine> for SendPayload<Engine>
where
    Engine: EngineTypes + PayloadTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let node_idx = self.config.node_idx.unwrap_or(env.active_node_idx);
            if node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index {} out of bounds", node_idx));
            }

            // Load and parse payload data
            let (payload, versioned_hashes) = match &self.source {
                PayloadSource::Node { source_node_idx, block_number } => {
                    // Handle node-based payload loading (similar to SendNewPayload logic)
                    if *source_node_idx >= env.node_clients.len() {
                        return Err(eyre::eyre!(
                            "Source node index {} out of bounds",
                            source_node_idx
                        ));
                    }

                    let source_rpc = &env.node_clients[*source_node_idx].rpc;
                    let mut block = None;
                    let mut retries = 0;

                    while retries < MAX_BLOCK_RETRIEVAL_RETRIES {
                        match EthApiClient::<TransactionRequest, Transaction, Block, Receipt, Header>::block_by_number(
                            source_rpc,
                            alloy_eips::BlockNumberOrTag::Number(*block_number),
                            true,
                        ).await {
                            Ok(Some(b)) => {
                                block = Some(b);
                                break;
                            }
                            Ok(None) => {
                                debug!(
                                    "Block {} not found on source node {} (attempt {}/{})",
                                    block_number, source_node_idx, retries + 1, MAX_BLOCK_RETRIEVAL_RETRIES
                                );
                                retries += 1;
                                if retries < MAX_BLOCK_RETRIEVAL_RETRIES {
                                    tokio::time::sleep(tokio::time::Duration::from_millis(BLOCK_RETRIEVAL_RETRY_DELAY_MS)).await;
                                }
                            }
                            Err(e) => return Err(e.into()),
                        }
                    }

                    let block = block.ok_or_else(|| {
                        eyre::eyre!(
                            "Block {} not found on source node {} after {} retries",
                            block_number,
                            source_node_idx,
                            MAX_BLOCK_RETRIEVAL_RETRIES
                        )
                    })?;

                    (block_to_payload_v3(block), vec![])
                }
                _ => {
                    // Handle JSON/file-based payload loading
                    let payload_data = load_payload_data(&self.source).await?;
                    let parsed = parse_payload_json(&payload_data)?;

                    match parsed {
                        PayloadJsonFormat::Array { block, versioned_hashes } => {
                            (json_to_payload_v3(&block)?, versioned_hashes)
                        }
                        PayloadJsonFormat::Object(block) => (json_to_payload_v3(&block)?, vec![]),
                    }
                }
            };

            // Send the payload
            let target_engine = env.node_clients[node_idx].engine.http_client();
            let result = EngineApiClient::<Engine>::new_payload_v3(
                &target_engine,
                payload,
                versioned_hashes,
                alloy_primitives::B256::ZERO,
            )
            .await?;

            debug!(
                "Node {}: new_payload response - status: {:?}, latest_valid_hash: {:?}",
                node_idx, result.status, result.latest_valid_hash
            );

            // Validate the response
            let context = match &self.source {
                PayloadSource::Json(_) => "JSON payload",
                PayloadSource::File(path) => &format!("file {}", path),
                PayloadSource::Node { block_number, .. } => &format!("block {}", block_number),
            };

            validate_payload_response(&result, &self.config.expected_status, node_idx, context)
        })
    }
}

/// Action to load and send a payload from embedded JSON data
#[derive(Debug)]
pub struct SendPayloadFromJson<Engine> {
    /// JSON payload data as string (can be from `include_str`! or dynamic)
    pub payload_json: String,
    /// Node index to send to
    pub node_idx: Option<usize>,
    /// Expected payload status
    pub expected_status: ExpectedPayloadStatus,
    _phantom: PhantomData<Engine>,
}

impl<Engine> SendPayloadFromJson<Engine> {
    /// Create a new `SendPayloadFromJson` action from embedded JSON string
    pub fn new(payload_json: impl Into<String>) -> Self {
        Self {
            payload_json: payload_json.into(),
            node_idx: None,
            expected_status: ExpectedPayloadStatus::Valid,
            _phantom: Default::default(),
        }
    }

    /// Create a new `SendPayloadFromJson` action from a file path (alternative to embedded)
    pub fn from_file(payload_file: impl Into<String>) -> Result<Self> {
        let payload_data = std::fs::read_to_string(payload_file.into())?;
        Ok(Self {
            payload_json: payload_data,
            node_idx: None,
            expected_status: ExpectedPayloadStatus::Valid,
            _phantom: Default::default(),
        })
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }

    /// Set expected status for the payload
    pub const fn with_expected_status(mut self, status: ExpectedPayloadStatus) -> Self {
        self.expected_status = status;
        self
    }
}

impl<Engine> Action<Engine> for SendPayloadFromJson<Engine>
where
    Engine: EngineTypes + PayloadTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Delegate to the unified SendPayload action
            let mut unified_sender = SendPayload::from_json(self.payload_json.clone());

            if let Some(node_idx) = self.node_idx {
                unified_sender = unified_sender.with_node_idx(node_idx);
            }

            unified_sender = unified_sender.with_expected_status(self.expected_status.clone());

            unified_sender.execute(env).await
        })
    }
}

/// Action to load and send a payload from a JSON file
#[derive(Debug)]
pub struct SendPayloadFromFile<Engine> {
    /// Path to the JSON payload file
    pub payload_file: String,
    /// Node index to send to
    pub node_idx: Option<usize>,
    /// Expected payload status
    pub expected_status: ExpectedPayloadStatus,
    _phantom: PhantomData<Engine>,
}

impl<Engine> SendPayloadFromFile<Engine> {
    /// Create a new `SendPayloadFromFile` action
    pub fn new(payload_file: impl Into<String>) -> Self {
        Self {
            payload_file: payload_file.into(),
            node_idx: None,
            expected_status: ExpectedPayloadStatus::Valid,
            _phantom: Default::default(),
        }
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }

    /// Set expected status for the payload
    pub const fn with_expected_status(mut self, status: ExpectedPayloadStatus) -> Self {
        self.expected_status = status;
        self
    }
}

impl<Engine> Action<Engine> for SendPayloadFromFile<Engine>
where
    Engine: EngineTypes + PayloadTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Delegate to the unified SendPayload action
            let mut unified_sender = SendPayload::from_file(self.payload_file.clone());

            if let Some(node_idx) = self.node_idx {
                unified_sender = unified_sender.with_node_idx(node_idx);
            }

            unified_sender = unified_sender.with_expected_status(self.expected_status.clone());

            unified_sender.execute(env).await
        })
    }
}

/// Helper function to convert JSON block data to `ExecutionPayloadV3`
fn json_to_payload_v3(block_json: &serde_json::Value) -> Result<ExecutionPayloadV3> {
    use alloy_primitives::{Address, B256, U256};

    // Extract fields from JSON
    let parent_hash: B256 = serde_json::from_value(
        block_json.get("parentHash").cloned().ok_or_else(|| eyre::eyre!("Missing parentHash"))?,
    )?;
    let fee_recipient: Address = serde_json::from_value(
        block_json
            .get("feeRecipient")
            .or_else(|| block_json.get("miner"))
            .cloned()
            .ok_or_else(|| eyre::eyre!("Missing feeRecipient or miner"))?,
    )?;
    let state_root: B256 = serde_json::from_value(
        block_json.get("stateRoot").cloned().ok_or_else(|| eyre::eyre!("Missing stateRoot"))?,
    )?;
    let receipts_root: B256 = serde_json::from_value(
        block_json
            .get("receiptsRoot")
            .cloned()
            .ok_or_else(|| eyre::eyre!("Missing receiptsRoot"))?,
    )?;
    let logs_bloom: alloy_primitives::Bloom = serde_json::from_value(
        block_json.get("logsBloom").cloned().ok_or_else(|| eyre::eyre!("Missing logsBloom"))?,
    )?;
    let prev_randao: B256 = serde_json::from_value(
        block_json
            .get("prevRandao")
            .or_else(|| block_json.get("mixHash"))
            .cloned()
            .ok_or_else(|| eyre::eyre!("Missing prevRandao or mixHash"))?,
    )?;
    // Parse hex strings to u64 for numeric fields - handle both Engine API and standard block
    // formats
    let block_number_str = block_json
        .get("blockNumber")
        .or_else(|| block_json.get("number"))
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Missing or invalid blockNumber/number"))?;
    let block_number = u64::from_str_radix(&block_number_str[2..], 16)
        .map_err(|e| eyre::eyre!("Failed to parse blockNumber '{}': {}", block_number_str, e))?;

    let gas_limit_str = block_json
        .get("gasLimit")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Missing or invalid gasLimit"))?;
    let gas_limit = u64::from_str_radix(&gas_limit_str[2..], 16)
        .map_err(|e| eyre::eyre!("Failed to parse gasLimit '{}': {}", gas_limit_str, e))?;

    let gas_used_str = block_json
        .get("gasUsed")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Missing or invalid gasUsed"))?;
    let gas_used = u64::from_str_radix(&gas_used_str[2..], 16)
        .map_err(|e| eyre::eyre!("Failed to parse gasUsed '{}': {}", gas_used_str, e))?;

    let timestamp_str = block_json
        .get("timestamp")
        .and_then(|v| v.as_str())
        .ok_or_else(|| eyre::eyre!("Missing or invalid timestamp"))?;
    let timestamp = u64::from_str_radix(&timestamp_str[2..], 16)
        .map_err(|e| eyre::eyre!("Failed to parse timestamp '{}': {}", timestamp_str, e))?;
    let extra_data: alloy_primitives::Bytes = serde_json::from_value(
        block_json.get("extraData").cloned().ok_or_else(|| eyre::eyre!("Missing extraData"))?,
    )?;
    let base_fee_per_gas: U256 = serde_json::from_value(
        block_json
            .get("baseFeePerGas")
            .cloned()
            .ok_or_else(|| eyre::eyre!("Missing baseFeePerGas"))?,
    )?;
    let block_hash: B256 = serde_json::from_value(
        block_json
            .get("blockHash")
            .or_else(|| block_json.get("hash"))
            .cloned()
            .ok_or_else(|| eyre::eyre!("Missing blockHash or hash"))?,
    )?;

    // Extract transactions - handle both Engine API (hex strings) and block format (objects)
    let transactions: Vec<alloy_primitives::Bytes> =
        if let Some(tx_json) = block_json.get("transactions") {
            if tx_json.is_array() {
                let tx_array = tx_json.as_array().unwrap();
                debug!("Transactions is array with {} items", tx_array.len());

                if tx_array.is_empty() {
                    vec![]
                } else if tx_array[0].is_string() {
                    // Engine API format: transactions as hex strings
                    debug!("Transactions are hex strings (Engine API format)");
                    serde_json::from_value(tx_json.clone())?
                } else if tx_array[0].is_object() {
                    // Block format: transactions as objects - return empty for now
                    debug!("Transactions are objects (block format) - using empty array");
                    vec![]
                } else {
                    return Err(eyre::eyre!("Unknown transaction array element type"));
                }
            } else if tx_json.is_string() {
                // Standard block format: transactions as single hex string
                debug!("Transactions is string format");
                vec![]
            } else {
                debug!("Transactions format: {:?}", tx_json);
                return Err(eyre::eyre!("Invalid transactions format: {:?}", tx_json));
            }
        } else {
            debug!("No transactions field found");
            vec![]
        };

    // Create ExecutionPayloadV3
    let payload = ExecutionPayloadV3 {
        payload_inner: ExecutionPayloadV2 {
            payload_inner: ExecutionPayloadV1 {
                parent_hash,
                fee_recipient,
                state_root,
                receipts_root,
                logs_bloom,
                prev_randao,
                block_number,
                gas_limit,
                gas_used,
                timestamp,
                extra_data,
                base_fee_per_gas,
                block_hash,
                transactions,
            },
            withdrawals: vec![], // No withdrawals in these payloads
        },
        blob_gas_used: 0,   // Not present in these payloads
        excess_blob_gas: 0, // Not present in these payloads
    };

    Ok(payload)
}
