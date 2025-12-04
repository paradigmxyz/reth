//! Engine API specific actions for testing.

use crate::testsuite::{Action, Environment};
use alloy_primitives::B256;
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
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Target node index out of bounds: {}", self.node_idx));
            }
            if self.source_node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!(
                    "Source node index out of bounds: {}",
                    self.source_node_idx
                ));
            }

            // Get the block from the source node with retries
            let source_rpc = &env.node_clients[self.source_node_idx].rpc;
            let mut block = None;
            let mut retries = 0;
            const MAX_RETRIES: u32 = 5;

            while retries < MAX_RETRIES {
                match EthApiClient::<TransactionRequest, Transaction, Block, Receipt, Header>::block_by_number(
                    source_rpc,
                    alloy_eips::BlockNumberOrTag::Number(self.block_number),
                    true, // include transactions
                )
                .await
                {
                    Ok(Some(b)) => {
                        block = Some(b);
                        break;
                    }
                    Ok(None) => {
                        debug!(
                            "Block {} not found on source node {} (attempt {}/{})",
                            self.block_number,
                            self.source_node_idx,
                            retries + 1,
                            MAX_RETRIES
                        );
                        retries += 1;
                        if retries < MAX_RETRIES {
                            tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                        }
                    }
                    Err(e) => return Err(e.into()),
                }
            }

            let block = block.ok_or_else(|| {
                eyre::eyre!(
                    "Block {} not found on source node {} after {} retries",
                    self.block_number,
                    self.source_node_idx,
                    MAX_RETRIES
                )
            })?;

            // Convert block to ExecutionPayloadV3
            let payload = block_to_payload_v3(block.clone());

            // Send the payload to the target node
            let target_engine = env.node_clients[self.node_idx].engine.http_client();
            let result = EngineApiClient::<Engine>::new_payload_v3(
                &target_engine,
                payload,
                vec![],
                B256::ZERO, // parent_beacon_block_root
            )
            .await?;

            debug!(
                "Node {}: new_payload for block {} response - status: {:?}, latest_valid_hash: {:?}",
                self.node_idx, self.block_number, result.status, result.latest_valid_hash
            );

            // Validate the response based on expectations
            match (&result.status, &self.expected_status) {
                (PayloadStatusEnum::Valid, ExpectedPayloadStatus::Valid) => {
                    debug!(
                        "Node {}: Block {} marked as VALID as expected",
                        self.node_idx, self.block_number
                    );
                    Ok(())
                }
                (
                    PayloadStatusEnum::Invalid { validation_error },
                    ExpectedPayloadStatus::Invalid,
                ) => {
                    debug!(
                        "Node {}: Block {} marked as INVALID as expected: {:?}",
                        self.node_idx, self.block_number, validation_error
                    );
                    Ok(())
                }
                (
                    PayloadStatusEnum::Syncing | PayloadStatusEnum::Accepted,
                    ExpectedPayloadStatus::SyncingOrAccepted,
                ) => {
                    debug!(
                        "Node {}: Block {} marked as SYNCING/ACCEPTED as expected (buffered)",
                        self.node_idx, self.block_number
                    );
                    Ok(())
                }
                (status, expected) => Err(eyre::eyre!(
                    "Node {}: Unexpected payload status for block {}. Got {:?}, expected {:?}",
                    self.node_idx,
                    self.block_number,
                    status,
                    expected
                )),
            }
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
