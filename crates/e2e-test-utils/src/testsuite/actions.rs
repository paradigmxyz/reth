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
use reth_node_api::EngineTypes;
use reth_rpc_api::clients::{EngineApiClient, EthApiClient};
use std::{future::Future, marker::PhantomData};
use tracing::debug;

/// An action that can be performed on an instance.
///
/// Actions execute operations and potentially make assertions in a single step.
/// The action name indicates what it does (e.g., `AssertMineBlock` would both
/// mine a block and assert it worked).
pub trait Action<I>: Send + 'static {
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<()>>;
}

/// Simplified action container for storage in tests
#[allow(missing_debug_implementations)]
pub struct ActionBox<I>(Box<dyn Action<I>>);

impl<I: 'static> ActionBox<I> {
    /// Constructor for [`ActionBox`].
    pub fn new<A: Action<I>>(action: A) -> Self {
        Self(Box::new(action))
    }

    /// Executes an [`ActionBox`] with the given [`Environment`] reference.
    pub async fn execute(mut self, env: &Environment<I>) -> Result<()> {
        self.0.execute(env).await
    }
}

/// Implementation of `Action` for any function/closure that takes an Environment
/// reference and returns a Future resolving to Result<()>.
///
/// This allows using closures directly as actions with `.with_action(async move |env| {...})`.
impl<I, F, Fut> Action<I> for F
where
    F: FnMut(&Environment<I>) -> Fut + Send + 'static,
    Fut: Future<Output = Result<()>> + Send + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for MineBlock {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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
pub struct AssertMineBlock<Engine> {
    /// The node index to mine
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Bytes>,
    /// Expected block hash (optional)
    pub expected_hash: Option<B256>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> AssertMineBlock<Engine> {
    /// Create a new `AssertMineBlock` action
    pub fn new(node_idx: usize, transactions: Vec<Bytes>, expected_hash: Option<B256>) -> Self {
        Self { node_idx, transactions, expected_hash, _phantom: Default::default() }
    }
}

impl<Engine> Action<Engine> for AssertMineBlock<Engine>
where
    Engine: EngineTypes,
    Engine::PayloadAttributes: From<PayloadAttributes>,
{
    fn execute<'a>(&'a mut self, env: &'a Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index out of bounds: {}", self.node_idx));
            }

            let node_client = &env.node_clients[self.node_idx];
            let rpc_client = &node_client.rpc;
            let engine_client = &node_client.engine;

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
                timestamp,
                prev_randao: B256::random(),
                suggested_fee_recipient: Address::random(),
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };

            let engine_payload_attributes: Engine::PayloadAttributes = payload_attributes.into();

            let fcu_result = EngineApiClient::<Engine>::fork_choice_updated_v3(
                engine_client,
                fork_choice_state,
                Some(engine_payload_attributes),
            )
            .await?;

            debug!("FCU result: {:?}", fcu_result);

            // check if we got a valid payload ID
            match fcu_result.payload_status.status {
                PayloadStatusEnum::Valid => {
                    if let Some(payload_id) = fcu_result.payload_id {
                        debug!("Got payload ID: {payload_id}");

                        // get the payload that was built
                        let _engine_payload =
                            EngineApiClient::<Engine>::get_payload_v3(engine_client, payload_id)
                                .await?;
                        Ok(())
                    } else {
                        Err(eyre::eyre!("No payload ID returned from forkchoiceUpdated"))
                    }
                }
                _ => Err(eyre::eyre!("Payload status not valid: {:?}", fcu_result.payload_status)),
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

impl<I: Sync> Action<I> for SubmitTransaction {
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for CreatePayload {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for NewPayload {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for GetBlock {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for GetTransactionCount {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for GetStorageAt {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for Call {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for WaitForBlocks {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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

impl<I> Action<I> for ChainReorg {
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // 1. Reorg the chain to the specified depth
            // 2. Add the new blocks

            Ok(())
        })
    }
}

/// Run a sequence of actions in series.
#[allow(missing_debug_implementations)]
pub struct Sequence<I> {
    /// Actions to execute in sequence
    pub actions: Vec<Box<dyn Action<I>>>,
}

impl<I> Sequence<I> {
    /// Create a new sequence of actions
    pub fn new(actions: Vec<Box<dyn Action<I>>>) -> Self {
        Self { actions }
    }
}

impl<I: Sync + 'static> Action<I> for Sequence<I> {
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
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
