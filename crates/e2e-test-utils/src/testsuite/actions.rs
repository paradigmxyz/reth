//! Actions that can be performed in tests.

use crate::testsuite::Environment;
use alloy_eips::BlockId;
use alloy_primitives::{Address, BlockNumber, Bytes, B256};
use alloy_rpc_types_engine::{
    ExecutionPayload, ForkchoiceState, PayloadAttributes, PayloadStatusEnum,
};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_ethereum::EthEngineTypes;
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::future::Future;
use tracing::debug;

/// An action that can be performed on an instance.
///
/// Actions execute operations and potentially make assertions in a single step.
/// The action name indicates what it does (e.g., `AssertMineBlock` would both
/// mine a block and assert it worked).
pub trait Action: Send + 'static {
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a Environment) -> BoxFuture<'a, Result<()>>;
}

/// Simplified action container for storage in tests
#[allow(missing_debug_implementations)]
pub struct ActionBox(Box<dyn Action>);

impl ActionBox {
    /// Constructor for [`ActionBox`].
    pub fn new<A: Action>(action: A) -> Self {
        Self(Box::new(action))
    }

    /// Executes an [`ActionBox`] with the given [`Environment`] reference.
    pub async fn execute(mut self, env: &Environment) -> Result<()> {
        self.0.execute(env).await
    }
}

/// Implementation of `Action` for any function/closure that takes an Environment
/// reference and returns a Future resolving to Result<()>.
///
/// This allows using closures directly as actions with `.with_action(async move |env| {...})`.
impl<F, Fut> Action for F
where
    F: FnMut(&Environment) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(self(env))
    }
}

/// Advance a single block with the given transactions.
#[derive(Debug)]
pub struct MineBlock {
    /// The node index to advance
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Vec<u8>>,
}

impl Action for MineBlock {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // 1. Create a new payload with the given transactions
            // 2. Execute forkchoiceUpdated with the new payload
            // 3. Internal implementation would go here

            Ok(())
        })
    }
}

/// Mine a single block with the given transactions and verify the block was created
/// successfully.
#[derive(Debug)]
pub struct AssertMineBlock {
    /// The node index to mine
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Bytes>,
    /// Expected block hash (optional)
    pub expected_hash: Option<B256>,
}

impl Action for AssertMineBlock {
    fn execute<'a>(&'a mut self, env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", self.node_idx));
            }

            let node_client = &env.node_clients[self.node_idx];
            let rpc_client = &node_client.rpc;
            let engine_client = &node_client.engine;

            // get current block number before mining
            let pre_block_number =
                EthApiClient::<Transaction, Block, Receipt, Header>::block_number(rpc_client)
                    .await?;

            debug!("Current block number: {pre_block_number}");

            // get the latest block to use as parent
            let latest_block =
                EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                    rpc_client,
                    alloy_eips::BlockNumberOrTag::Latest,
                    false,
                )
                .await?;

            let latest_block = latest_block.ok_or_else(|| eyre::eyre!("Latest block not found"))?;
            let parent_hash = latest_block.header.hash;

            debug!("Latest block hash: {parent_hash}");

            // create a simple forkchoice state with the latest block as head
            let fork_choice_state = ForkchoiceState {
                head_block_hash: parent_hash,
                safe_block_hash: parent_hash,
                finalized_block_hash: parent_hash,
            };

            let timestamp = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs();

            // create payload attributes for the new block
            let payload_attributes = PayloadAttributes {
                timestamp: timestamp.into(),
                prev_randao: B256::random(),
                suggested_fee_recipient: Address::random(),
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            debug!("Creating payload with timestamp: {timestamp}");

            let fcu_result = EngineApiClient::<EthEngineTypes>::fork_choice_updated_v3(
                engine_client,
                fork_choice_state,
                Some(payload_attributes),
            )
            .await?;

            debug!("FCU result: {:?}", fcu_result);

            // check if we got a valid payload ID
            match fcu_result.payload_status.status {
                PayloadStatusEnum::Valid => {
                    if let Some(payload_id) = fcu_result.payload_id {
                        debug!("Got payload ID: {payload_id}");

                        // get the payload that was built
                        let payload =
                            EngineApiClient::get_payload_v3(engine_client, payload_id).await?;

                        // execute the new payload
                        let new_payload_result = EngineApiClient::new_payload_v3(
                            engine_client,
                            payload,
                            Vec::new(),
                            B256::ZERO,
                        )
                        .await?;

                        debug!("New payload result: {:?}", new_payload_result);

                        if new_payload_result.status != PayloadStatusEnum::Valid {
                            return Err(eyre::eyre!(
                                "New payload was not valid: {:?}",
                                new_payload_result
                            ));
                        }

                        // update the forkchoice state to set the new head
                        let new_fork_choice_state = ForkchoiceState {
                            head_block_hash: payload.payload_inner.payload_inner.block_hash,
                            safe_block_hash: payload.payload_inner.payload_inner.block_hash,
                            finalized_block_hash: parent_hash,
                        };

                        let final_fcu_result = EngineApiClient::fork_choice_updated_v3(
                            engine_client,
                            new_fork_choice_state,
                            None,
                        )
                        .await?;

                        debug!("Final FCU result: {:?}", final_fcu_result);

                        // verify the block was actually mined
                        let post_block_number =
                            EthApiClient::<Transaction, Block, Receipt, Header>::block_number(
                                rpc_client,
                            )
                            .await?;

                        debug!("New block number: {post_block_number}");

                        if post_block_number <= pre_block_number {
                            return Err(eyre::eyre!(
                                "Block number did not increase: pre={}, post={}",
                                pre_block_number,
                                post_block_number
                            ));
                        }

                        // if an expected hash was provided, verify it
                        if let Some(expected_hash) = self.expected_hash {
                            if payload.payload_inner.payload_inner.block_hash != expected_hash {
                                return Err(eyre::eyre!(
                                    "Block hash mismatch: expected={}, actual={}",
                                    expected_hash,
                                    payload.payload_inner.payload_inner.block_hash
                                ));
                            }
                        }

                        debug!(
                            "Successfully mined a block: number={}, hash={}",
                            post_block_number, payload.payload_inner.payload_inner.block_hash
                        );
                        return Ok(());
                    }
                    return Err(eyre::eyre!("No payload ID returned from forkchoiceUpdated"));
                }
                _ => {
                    return Err(eyre::eyre!(
                        "Payload status not valid: {:?}",
                        fcu_result.payload_status
                    ));
                }
            }
        })
    }
}

/// Submit a transaction to the node.
#[derive(Debug)]
pub struct SubmitTransaction {
    /// The node index to submit to
    pub node_idx: usize,
    /// The raw transaction bytes
    pub raw_tx: Bytes,
}

impl Action for SubmitTransaction {
    fn execute<'a>(&'a mut self, env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", self.node_idx));
            }

            let node_client = &env.node_clients[self.node_idx];
            let rpc_client = &node_client.rpc;

            let tx_hash =
                EthApiClient::<Transaction, Block, Receipt, Header>::send_raw_transaction(
                    rpc_client,
                    self.raw_tx.clone(),
                )
                .await?;

            debug!("Transaction submitted with hash: {}", tx_hash);

            Ok(())
        })
    }
}

/// Create a new payload with specified attributes.
#[derive(Debug)]
pub struct CreatePayload {
    /// The node index to use
    pub node_idx: usize,
    /// Payload attributes
    pub attributes: PayloadAttributes,
}

impl Action for CreatePayload {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Execute newPayload with the given payload.
#[derive(Debug)]
pub struct NewPayload {
    /// The node index to use
    pub node_idx: usize,
    /// Execution payload
    pub payload: ExecutionPayload,
}

impl Action for NewPayload {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Get a block by number or hash.
#[derive(Debug)]
pub struct GetBlock {
    /// The node index to use
    pub node_idx: usize,
    /// Block number
    pub block_number: Option<BlockNumber>,
    /// Block hash
    pub block_hash: Option<B256>,
}

impl Action for GetBlock {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Get the transaction count for an address.
#[derive(Debug)]
pub struct GetTransactionCount {
    /// The node index to use
    pub node_idx: usize,
    /// Account address
    pub address: B256,
    /// Block identifier
    pub block_id: BlockId,
}

impl Action for GetTransactionCount {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Get the storage value at a specific slot.
#[derive(Debug)]
pub struct GetStorageAt {
    /// The node index to use
    pub node_idx: usize,
    /// Account address
    pub address: B256,
    /// Storage slot
    pub slot: B256,
    /// Block Identifier
    pub block_id: BlockId,
}

impl Action for GetStorageAt {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Call a contract method.
#[derive(Debug)]
pub struct Call {
    /// The node index to use
    pub node_idx: usize,
    /// Call request (tx data)
    pub request: Vec<u8>,
    /// Block identifier
    pub block_id: BlockId,
}

impl Action for Call {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Wait for a specified number of blocks.
#[derive(Debug)]
pub struct WaitForBlocks {
    /// The node index to use
    pub node_idx: usize,
    /// Number of blocks to wait for
    pub blocks: u64,
}

impl Action for WaitForBlocks {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
    }
}

/// Chain reorg action to test reorgs.
#[derive(Debug)]
pub struct ChainReorg {
    /// The node index to use
    pub node_idx: usize,
    /// Number of blocks to reorg
    pub depth: u64,
    /// New blocks to add
    pub new_blocks: Vec<Vec<u8>>,
}

impl Action for ChainReorg {
    fn execute<'a>(&'a mut self, _env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // 1. Reorg the chain to the specified depth
            // 2. Add the new blocks

            Ok(())
        })
    }
}

/// Run a sequence of actions in series.
#[allow(missing_debug_implementations)]
pub struct Sequence {
    /// Actions to execute in sequence
    pub actions: Vec<Box<dyn Action>>,
}

impl Sequence {
    /// Create a new sequence of actions
    pub fn new(actions: Vec<Box<dyn Action>>) -> Self {
        Self { actions }
    }
}

impl Action for Sequence {
    fn execute<'a>(&'a mut self, env: &'a Environment) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Execute each action in sequence
            futures_util::future::try_join_all(
                self.actions.iter_mut().map(|action| action.execute(env)),
            )
            .await?;

            Ok(())
        })
    }
}
