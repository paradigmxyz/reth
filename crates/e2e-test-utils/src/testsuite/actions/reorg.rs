//! Reorg actions for the e2e testing framework.

use crate::testsuite::{
    actions::{
        produce_blocks::{BroadcastLatestForkchoice, UpdateBlockInfo},
        Action, Sequence,
    },
    Environment, LatestBlockInfo,
};
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::EthApiClient;
use std::marker::PhantomData;
use tracing::debug;

/// Target for reorg operation
#[derive(Debug, Clone)]
pub enum ReorgTarget {
    /// Direct block hash
    Hash(B256),
    /// Tagged block reference
    Tag(String),
}

/// Action that performs a reorg by setting a new head block as canonical
#[derive(Debug)]
pub struct ReorgTo<Engine> {
    /// Target for the reorg operation
    pub target: ReorgTarget,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> ReorgTo<Engine> {
    /// Create a new `ReorgTo` action with a direct block hash
    pub const fn new(target_hash: B256) -> Self {
        Self { target: ReorgTarget::Hash(target_hash), _phantom: PhantomData }
    }

    /// Create a new `ReorgTo` action with a tagged block reference
    pub fn new_from_tag(tag: impl Into<String>) -> Self {
        Self { target: ReorgTarget::Tag(tag.into()), _phantom: PhantomData }
    }
}

impl<Engine> Action<Engine> for ReorgTo<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3:
        Into<alloy_rpc_types_engine::payload::ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // resolve the target hash from either direct hash or tag
            let target_hash = match &self.target {
                ReorgTarget::Hash(hash) => *hash,
                ReorgTarget::Tag(tag) => env
                    .block_registry
                    .get(tag)
                    .copied()
                    .ok_or_else(|| eyre::eyre!("Block tag '{}' not found in registry", tag))?,
            };

            let mut sequence = Sequence::new(vec![
                Box::new(SetReorgTarget::new(target_hash)),
                Box::new(BroadcastLatestForkchoice::default()),
                Box::new(UpdateBlockInfo::default()),
            ]);

            sequence.execute(env).await
        })
    }
}

/// Sub-action to set the reorg target block in the environment
#[derive(Debug)]
pub struct SetReorgTarget {
    /// Hash of the block to reorg to
    pub target_hash: B256,
}

impl SetReorgTarget {
    /// Create a new `SetReorgTarget` action
    pub const fn new(target_hash: B256) -> Self {
        Self { target_hash }
    }
}

impl<Engine> Action<Engine> for SetReorgTarget
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if env.node_clients.is_empty() {
                return Err(eyre::eyre!("No node clients available"));
            }

            // verify the target block exists
            let rpc_client = &env.node_clients[0].rpc;
            let target_block = EthApiClient::<Transaction, Block, Receipt, Header>::block_by_hash(
                rpc_client,
                self.target_hash,
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Target reorg block {} not found", self.target_hash))?;

            debug!(
                "Setting reorg target to block {} (hash: {})",
                target_block.header.number, self.target_hash
            );

            // update environment to point to the target block
            env.latest_block_info = Some(LatestBlockInfo {
                hash: target_block.header.hash,
                number: target_block.header.number,
            });

            env.latest_header_time = target_block.header.timestamp;

            // update fork choice state to make the target block canonical
            env.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: self.target_hash,
                safe_block_hash: self.target_hash, // Simplified - in practice might be different
                finalized_block_hash: self.target_hash, /* Simplified - in practice might be
                                                    * different */
            };

            debug!("Set reorg target to block {}", self.target_hash);
            Ok(())
        })
    }
}
