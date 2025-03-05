//! Actions that can be performed in tests.

use crate::testsuite::Environment;
use alloy_eips::BlockId;
use alloy_primitives::{BlockNumber, B256};
use alloy_rpc_types_engine::{ExecutionPayload, PayloadAttributes};
use eyre::Result;
use futures_util::future::BoxFuture;
use std::future::Future;

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
    I: Send + Sync + 'static,
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

impl<I> Action<I> for MineBlock
where
    I: Send + Sync + 'static,
{
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
pub struct AssertMineBlock {
    /// The node index to mine
    pub node_idx: usize,
    /// Transactions to include in the block
    pub transactions: Vec<Vec<u8>>,
    /// Expected block hash (optional)
    pub expected_hash: Option<B256>,
}

impl<I> Action<I> for AssertMineBlock
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // 1. Create a new payload with the given transactions
            // 2. Execute forkchoiceUpdated with the new payload
            // 3. Verify the block was created successfully
            // 4. If expected_hash is provided, verify the block hash matches

            /*
             * Example assertion code (would actually fetch the real hash):
            if let Some(expected_hash) = self.expected_hash {
                let actual_hash = B256::ZERO;
                if actual_hash != expected_hash {
                    return Err(eyre!(
                        "Block hash mismatch: expected {}, got {}",
                        expected_hash,
                        actual_hash
                    ));
                }
            }
            */

            Ok(())
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
}

impl<I> Action<I> for SubmitTransaction
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
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

impl<I> Action<I> for CreatePayload
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, _env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move { Ok(()) })
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
}

impl<I> Action<I> for ForkchoiceUpdated
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for NewPayload
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for GetBlock
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for GetTransactionCount
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for GetStorageAt
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for Call
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for WaitForBlocks
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for ChainReorg
where
    I: Send + Sync + 'static,
{
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

impl<I> Action<I> for Sequence<I>
where
    I: Send + Sync + 'static,
{
    fn execute<'a>(&'a mut self, env: &'a Environment<I>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // Execute each action in sequence
            for action in &mut self.actions {
                action.execute(env).await?;
            }

            Ok(())
        })
    }
}
