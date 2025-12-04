//! Custom forkchoice update actions for testing specific FCU scenarios.

use crate::testsuite::{Action, Environment};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadStatusEnum};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::EngineTypes;
use reth_rpc_api::clients::EngineApiClient;
use std::marker::PhantomData;
use tracing::debug;

/// Reference to a block for forkchoice update
#[derive(Debug, Clone)]
pub enum BlockReference {
    /// Direct block hash
    Hash(B256),
    /// Tagged block reference
    Tag(String),
    /// Latest block on the active node
    Latest,
}

/// Helper function to resolve a block reference to a hash
pub fn resolve_block_reference<Engine: EngineTypes>(
    reference: &BlockReference,
    env: &Environment<Engine>,
) -> Result<B256> {
    match reference {
        BlockReference::Hash(hash) => Ok(*hash),
        BlockReference::Tag(tag) => {
            let (block_info, _) = env
                .block_registry
                .get(tag)
                .ok_or_else(|| eyre::eyre!("Block tag '{tag}' not found in registry"))?;
            Ok(block_info.hash)
        }
        BlockReference::Latest => {
            let block_info = env
                .current_block_info()
                .ok_or_else(|| eyre::eyre!("No current block information available"))?;
            Ok(block_info.hash)
        }
    }
}

/// Action to send a custom forkchoice update with specific finalized, safe, and head blocks
#[derive(Debug)]
pub struct SendForkchoiceUpdate<Engine> {
    /// The finalized block reference
    pub finalized: BlockReference,
    /// The safe block reference
    pub safe: BlockReference,
    /// The head block reference
    pub head: BlockReference,
    /// Expected payload status (None means accept any non-error)
    pub expected_status: Option<PayloadStatusEnum>,
    /// Node index to send to (None means active node)
    pub node_idx: Option<usize>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> SendForkchoiceUpdate<Engine> {
    /// Create a new custom forkchoice update action
    pub const fn new(
        finalized: BlockReference,
        safe: BlockReference,
        head: BlockReference,
    ) -> Self {
        Self { finalized, safe, head, expected_status: None, node_idx: None, _phantom: PhantomData }
    }

    /// Set expected status for the FCU response
    pub fn with_expected_status(mut self, status: PayloadStatusEnum) -> Self {
        self.expected_status = Some(status);
        self
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }
}

impl<Engine> Action<Engine> for SendForkchoiceUpdate<Engine>
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let finalized_hash = resolve_block_reference(&self.finalized, env)?;
            let safe_hash = resolve_block_reference(&self.safe, env)?;
            let head_hash = resolve_block_reference(&self.head, env)?;

            let fork_choice_state = ForkchoiceState {
                head_block_hash: head_hash,
                safe_block_hash: safe_hash,
                finalized_block_hash: finalized_hash,
            };

            debug!(
                "Sending FCU - finalized: {finalized_hash}, safe: {safe_hash}, head: {head_hash}"
            );

            let node_idx = self.node_idx.unwrap_or(env.active_node_idx);
            if node_idx >= env.node_clients.len() {
                return Err(eyre::eyre!("Node index {node_idx} out of bounds"));
            }

            let engine = env.node_clients[node_idx].engine.http_client();
            let fcu_response =
                EngineApiClient::<Engine>::fork_choice_updated_v3(&engine, fork_choice_state, None)
                    .await?;

            debug!(
                "Node {node_idx}: FCU response - status: {:?}, latest_valid_hash: {:?}",
                fcu_response.payload_status.status, fcu_response.payload_status.latest_valid_hash
            );

            // If we have an expected status, validate it
            if let Some(expected) = &self.expected_status {
                match (&fcu_response.payload_status.status, expected) {
                    (PayloadStatusEnum::Valid, PayloadStatusEnum::Valid) => {
                        debug!("Node {node_idx}: FCU returned VALID as expected");
                    }
                    (
                        PayloadStatusEnum::Invalid { validation_error },
                        PayloadStatusEnum::Invalid { .. },
                    ) => {
                        debug!(
                            "Node {node_idx}: FCU returned INVALID as expected: {validation_error:?}"
                        );
                    }
                    (PayloadStatusEnum::Syncing, PayloadStatusEnum::Syncing) => {
                        debug!("Node {node_idx}: FCU returned SYNCING as expected");
                    }
                    (PayloadStatusEnum::Accepted, PayloadStatusEnum::Accepted) => {
                        debug!("Node {node_idx}: FCU returned ACCEPTED as expected");
                    }
                    (actual, expected) => {
                        return Err(eyre::eyre!(
                            "Node {node_idx}: FCU status mismatch. Expected {expected:?}, got {actual:?}"
                        ));
                    }
                }
            } else {
                // Just validate it's not an error
                if matches!(fcu_response.payload_status.status, PayloadStatusEnum::Invalid { .. }) {
                    return Err(eyre::eyre!(
                        "Node {node_idx}: FCU returned unexpected INVALID status: {:?}",
                        fcu_response.payload_status.status
                    ));
                }
            }

            Ok(())
        })
    }
}

/// Action to finalize a specific block with a given head
#[derive(Debug)]
pub struct FinalizeBlock<Engine> {
    /// Block to finalize
    pub block_to_finalize: BlockReference,
    /// Current head block (if None, uses the finalized block)
    pub head: Option<BlockReference>,
    /// Node index to send to (None means active node)
    pub node_idx: Option<usize>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> FinalizeBlock<Engine> {
    /// Create a new finalize block action
    pub const fn new(block_to_finalize: BlockReference) -> Self {
        Self { block_to_finalize, head: None, node_idx: None, _phantom: PhantomData }
    }

    /// Set the head block (if different from finalized)
    pub fn with_head(mut self, head: BlockReference) -> Self {
        self.head = Some(head);
        self
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }
}

impl<Engine> Action<Engine> for FinalizeBlock<Engine>
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let finalized_hash = resolve_block_reference(&self.block_to_finalize, env)?;
            let head_hash = if let Some(ref head_ref) = self.head {
                resolve_block_reference(head_ref, env)?
            } else {
                finalized_hash
            };

            // Use SendForkchoiceUpdate to do the actual work
            let mut fcu_action = SendForkchoiceUpdate::new(
                BlockReference::Hash(finalized_hash),
                BlockReference::Hash(finalized_hash), // safe = finalized
                BlockReference::Hash(head_hash),
            );

            if let Some(idx) = self.node_idx {
                fcu_action = fcu_action.with_node_idx(idx);
            }

            fcu_action.execute(env).await?;

            debug!("Block {finalized_hash} successfully finalized with head at {head_hash}");

            Ok(())
        })
    }
}
