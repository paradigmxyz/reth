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

/// Action to test if reth allows reorgs that would change blocks behind finalized.
///
/// This tests the specific scenario:
/// 1. We have a chain: genesis -> ... -> `block_A` -> `block_B` -> `block_C`
/// 2. We finalize `block_B` (with head at `block_C`)
/// 3. We try to reorg to a different fork that branches before `block_B`
///
/// According to consensus rules, once `block_B` is finalized, we cannot accept
/// any fork that doesn't include `block_B` in its history.
#[derive(Debug)]
pub struct TestReorgBehindFinalized<Engine> {
    /// Block to finalize first
    pub block_to_finalize: BlockReference,
    /// Current head when finalizing
    pub current_head: BlockReference,
    /// Fork block that branches before finalized (attempting invalid reorg)
    pub fork_block: BlockReference,
    /// Node index to send to
    pub node_idx: Option<usize>,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> TestReorgBehindFinalized<Engine> {
    /// Create a new test for reorg behind finalized
    pub const fn new(
        block_to_finalize: BlockReference,
        current_head: BlockReference,
        fork_block: BlockReference,
    ) -> Self {
        Self { block_to_finalize, current_head, fork_block, node_idx: None, _phantom: PhantomData }
    }

    /// Set the target node index
    pub const fn with_node_idx(mut self, idx: usize) -> Self {
        self.node_idx = Some(idx);
        self
    }
}

impl<Engine> Action<Engine> for TestReorgBehindFinalized<Engine>
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let finalized_hash = resolve_block_reference(&self.block_to_finalize, env)?;
            let head_hash = resolve_block_reference(&self.current_head, env)?;
            let fork_hash = resolve_block_reference(&self.fork_block, env)?;

            let node_idx = self.node_idx.unwrap_or(env.active_node_idx);
            let engine = env.node_clients[node_idx].engine.http_client();

            // Step 1: Finalize a block with proper head
            let fork_choice_state = ForkchoiceState {
                head_block_hash: head_hash,
                safe_block_hash: finalized_hash,
                finalized_block_hash: finalized_hash,
            };

            debug!(
                "Node {node_idx}: Setting finalized={finalized_hash}, safe={finalized_hash}, head={head_hash}"
            );

            let fcu_response =
                EngineApiClient::<Engine>::fork_choice_updated_v3(&engine, fork_choice_state, None)
                    .await?;

            // This should be valid
            if !matches!(fcu_response.payload_status.status, PayloadStatusEnum::Valid) {
                return Err(eyre::eyre!(
                    "Node {node_idx}: Failed to finalize block. Status: {:?}",
                    fcu_response.payload_status.status
                ));
            }

            debug!(
                "Node {node_idx}: Block {finalized_hash} successfully finalized with head at {head_hash}"
            );

            // Step 2: Try to reorg to a fork that branches before the finalized block
            // This should be rejected because it would require changing the finalized block
            let reorg_fork_choice_state = ForkchoiceState {
                head_block_hash: fork_hash,
                safe_block_hash: fork_hash,
                finalized_block_hash: finalized_hash, // Keep finalized the same
            };

            debug!(
                "Node {node_idx}: Attempting reorg to fork {fork_hash} while keeping finalized at {finalized_hash}"
            );

            let reorg_response = EngineApiClient::<Engine>::fork_choice_updated_v3(
                &engine,
                reorg_fork_choice_state,
                None,
            )
            .await?;

            // This should return INVALID because the fork doesn't contain the finalized block
            match &reorg_response.payload_status.status {
                PayloadStatusEnum::Invalid { validation_error } => {
                    debug!(
                        "Node {node_idx}: Reorg correctly rejected with error: {validation_error}"
                    );
                    Ok(())
                }
                other_status => {
                    Err(eyre::eyre!(
                        "Node {node_idx}: Expected FCU to return INVALID for reorg to fork not containing finalized block, but got {other_status:?}"
                    ))
                }
            }
        })
    }
}
