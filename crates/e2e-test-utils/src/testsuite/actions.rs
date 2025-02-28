//! Actions that can be performed in tests.

use crate::testsuite::{BoxFuture, Environment};
use alloy_eips::BlockId;
use alloy_primitives::{BlockNumber, B256, U256};
use alloy_rpc_types_engine::{ExecutionPayload, PayloadAttributes};
use eyre::Result;
use std::sync::Arc;

/// An action that can be performed on an instance.
///
/// Actions can produce results that are stored for later assertions.
pub trait Action<I>: Send + 'static {
    /// Executes the action
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<ActionResult>>;
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
    pub async fn execute(mut self, env: &Environment<I>) -> Result<ActionResult> {
        self.0.execute(env).await
    }
}

/// Possible results from actions
#[derive(Debug)]
pub enum ActionResult {
    /// No result to store
    None,
    /// Store a payload with its identifier
    Payload(String, Arc<ExecutionPayload>),
    /// Store a block hash with its identifier
    BlockHash(String, B256),
    /// Store a transaction hash with its identifier
    TransactionHash(String, B256),
    /// Store a generic value with its identifier
    Value(String, U256),
    /// Store a boolean result with its identifier
    Bool(String, bool),
}

/// Advance a single block with the given transactions.
#[derive(Debug)]
pub struct AdvanceBlock {
    /// The node index to advance
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Vec<u8>>,
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for AdvanceBlock
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Create a new payload with the given transactions
            // 2. Execute forkchoiceUpdated with the new payload
            // 3. Return the payload ID and block hash

            Ok(ActionResult::None)
        })
    }
}

/// Submit a transaction to the node.
#[derive(Debug)]
pub struct SubmitTransaction {
    /// The node index to submit to
    pub node_idx: usize,
    /// The raw transaction bytes
    pub raw_tx: Vec<u8>,
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for SubmitTransaction
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Submit the transaction to the node
            // 2. Return the transaction hash

            Ok(ActionResult::None)
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for CreatePayload
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Create a new payload with the given attributes
            // 2. Return the payload

            Ok(ActionResult::None)
        })
    }
}

/// Execute forkchoiceUpdated with the given state.
#[derive(Debug)]
pub struct ForkchoiceUpdated {
    /// The node index to use
    pub node_idx: usize,
    /// Forkchoice state (head, safe, finalized block hashes)
    pub state: (B256, B256, B256),
    /// Payload attributes (optional)
    pub attributes: Option<PayloadAttributes>,
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for ForkchoiceUpdated
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Execute forkchoiceUpdated with the given state and attributes
            // 2. Return the result

            Ok(ActionResult::None)
        })
    }
}

/// Execute newPayload with the given payload.
#[derive(Debug)]
pub struct NewPayload {
    /// The node index to use
    pub node_idx: usize,
    /// Execution payload
    pub payload: ExecutionPayload,
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for NewPayload
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Execute newPayload with the given payload
            // 2. Return the result

            // For now, return a placeholder
            Ok(ActionResult::None)
        })
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for GetBlock
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Get the block by number or hash
            // 2. Return the block

            Ok(ActionResult::None)
        })
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for GetTransactionCount
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Get the transaction count for the address
            // 2. Return the count

            Ok(ActionResult::None)
        })
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for GetStorageAt
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Get the storage value at the slot
            // 2. Return the value

            Ok(ActionResult::None)
        })
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for Call
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Execute the call
            // 2. Return the result

            Ok(ActionResult::None)
        })
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

impl<I> Action<I> for WaitForBlocks
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Wait for the specified number of blocks
            // 2. Return nothing

            Ok(ActionResult::None)
        })
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
    /// Identifier for storing the result
    pub result_id: String,
}

impl<I> Action<I> for ChainReorg
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // 1. Reorg the chain to the specified depth
            // 2. Add the new blocks
            // 3. Return the new head

            Ok(ActionResult::None)
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

impl<I> Action<I> for Sequence<I>
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<ActionResult>> {
        Box::pin(async move {
            // Execute each action in sequence
            for action in &mut self.actions {
                action.execute(env).await?;
            }

            Ok(ActionResult::None)
        })
    }
}
