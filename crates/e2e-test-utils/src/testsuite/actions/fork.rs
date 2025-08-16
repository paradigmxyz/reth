//! Fork creation actions for the e2e testing framework.

use crate::testsuite::{
    actions::{produce_blocks::ProduceBlocks, Sequence},
    Action, BlockInfo, Environment,
};
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction, TransactionRequest};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::{EngineTypes, PayloadTypes};
use reth_rpc_api::clients::EthApiClient;
use std::marker::PhantomData;
use tracing::debug;

/// Fork base target for fork creation
#[derive(Debug, Clone)]
pub enum ForkBase {
    /// Block number
    Number(u64),
    /// Tagged block reference
    Tag(String),
}

/// Action to create a fork from a specified block and produce blocks on top
#[derive(Debug)]
pub struct CreateFork<Engine> {
    /// Fork base specification (either block number or tag)
    pub fork_base: ForkBase,
    /// Number of blocks to produce on top of the fork base
    pub num_blocks: u64,
    /// Tracks engine type
    _phantom: PhantomData<Engine>,
}

impl<Engine> CreateFork<Engine> {
    /// Create a new `CreateFork` action from a block number
    pub fn new(fork_base_block: u64, num_blocks: u64) -> Self {
        Self {
            fork_base: ForkBase::Number(fork_base_block),
            num_blocks,
            _phantom: Default::default(),
        }
    }

    /// Create a new `CreateFork` action from a tagged block
    pub fn new_from_tag(tag: impl Into<String>, num_blocks: u64) -> Self {
        Self { fork_base: ForkBase::Tag(tag.into()), num_blocks, _phantom: Default::default() }
    }
}

impl<Engine> Action<Engine> for CreateFork<Engine>
where
    Engine: EngineTypes + PayloadTypes,
    Engine::PayloadAttributes: From<PayloadAttributes> + Clone,
    Engine::ExecutionPayloadEnvelopeV3:
        Into<alloy_rpc_types_engine::payload::ExecutionPayloadEnvelopeV3>,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            // resolve the fork base and execute the appropriate sequence
            match &self.fork_base {
                ForkBase::Number(block_number) => {
                    // store the fork base for later validation on the active node
                    env.active_node_state_mut()?.current_fork_base = Some(*block_number);

                    let mut sequence = Sequence::new(vec![
                        Box::new(SetForkBase::new(*block_number)),
                        Box::new(ProduceBlocks::new(self.num_blocks)),
                    ]);
                    sequence.execute(env).await
                }
                ForkBase::Tag(tag) => {
                    let (block_info, _node_idx) =
                        env.block_registry.get(tag).copied().ok_or_else(|| {
                            eyre::eyre!("Block tag '{}' not found in registry", tag)
                        })?;

                    // store the fork base for later validation on the active node
                    env.active_node_state_mut()?.current_fork_base = Some(block_info.number);

                    let mut sequence = Sequence::new(vec![
                        Box::new(SetForkBaseFromBlockInfo::new(block_info)),
                        Box::new(ProduceBlocks::new(self.num_blocks)),
                    ]);
                    sequence.execute(env).await
                }
            }
        })
    }
}

/// Sub-action to set the fork base block in the environment
#[derive(Debug)]
pub struct SetForkBase {
    /// Block number to use as the base of the fork
    pub fork_base_block: u64,
}

/// Sub-action to set the fork base block from existing block info
#[derive(Debug)]
pub struct SetForkBaseFromBlockInfo {
    /// Complete block info to use as the base of the fork
    pub fork_base_info: BlockInfo,
}

impl SetForkBase {
    /// Create a new `SetForkBase` action
    pub const fn new(fork_base_block: u64) -> Self {
        Self { fork_base_block }
    }
}

impl SetForkBaseFromBlockInfo {
    /// Create a new `SetForkBaseFromBlockInfo` action
    pub const fn new(fork_base_info: BlockInfo) -> Self {
        Self { fork_base_info }
    }
}

impl<Engine> Action<Engine> for SetForkBase
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if env.node_clients.is_empty() {
                return Err(eyre::eyre!("No node clients available"));
            }

            // get the block at the fork base number to establish the fork point
            let rpc_client = &env.node_clients[0].rpc;
            let fork_base_block = EthApiClient::<
                TransactionRequest,
                Transaction,
                Block,
                Receipt,
                Header,
            >::block_by_number(
                rpc_client,
                alloy_eips::BlockNumberOrTag::Number(self.fork_base_block),
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Fork base block {} not found", self.fork_base_block))?;

            // update active node state to point to the fork base block
            let active_node_state = env.active_node_state_mut()?;
            active_node_state.current_block_info = Some(BlockInfo {
                hash: fork_base_block.header.hash,
                number: fork_base_block.header.number,
                timestamp: fork_base_block.header.timestamp,
            });

            active_node_state.latest_header_time = fork_base_block.header.timestamp;

            // update fork choice state to the fork base
            active_node_state.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: fork_base_block.header.hash,
                safe_block_hash: fork_base_block.header.hash,
                finalized_block_hash: fork_base_block.header.hash,
            };

            debug!(
                "Set fork base to block {} (hash: {})",
                self.fork_base_block, fork_base_block.header.hash
            );

            Ok(())
        })
    }
}

impl<Engine> Action<Engine> for SetForkBaseFromBlockInfo
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let block_info = self.fork_base_info;

            debug!(
                "Set fork base from block info: block {} (hash: {})",
                block_info.number, block_info.hash
            );

            // update active node state to point to the fork base block
            let active_node_state = env.active_node_state_mut()?;
            active_node_state.current_block_info = Some(block_info);
            active_node_state.latest_header_time = block_info.timestamp;

            // update fork choice state to the fork base
            active_node_state.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: block_info.hash,
                safe_block_hash: block_info.hash,
                finalized_block_hash: block_info.hash,
            };

            debug!("Set fork base to block {} (hash: {})", block_info.number, block_info.hash);

            Ok(())
        })
    }
}

/// Sub-action to validate that a fork was created correctly
#[derive(Debug)]
pub struct ValidateFork {
    /// Number of the fork base block (stored here since we need it for validation)
    pub fork_base_number: u64,
}

impl ValidateFork {
    /// Create a new `ValidateFork` action
    pub const fn new(fork_base_number: u64) -> Self {
        Self { fork_base_number }
    }
}

impl<Engine> Action<Engine> for ValidateFork
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let current_block_info = env
                .current_block_info()
                .ok_or_else(|| eyre::eyre!("No current block information available"))?;

            // verify that the current tip is at or ahead of the fork base
            if current_block_info.number < self.fork_base_number {
                return Err(eyre::eyre!(
                    "Fork validation failed: current block number {} is behind fork base {}",
                    current_block_info.number,
                    self.fork_base_number
                ));
            }

            // get the fork base hash from the environment's fork choice state
            // we assume the fork choice state was set correctly by SetForkBase
            let fork_base_hash =
                env.active_node_state()?.latest_fork_choice_state.finalized_block_hash;

            // trace back from current tip to verify it's a descendant of the fork base
            let rpc_client = &env.node_clients[0].rpc;
            let mut current_hash = current_block_info.hash;
            let mut current_number = current_block_info.number;

            // walk backwards through the chain until we reach the fork base
            while current_number > self.fork_base_number {
                let block = EthApiClient::<TransactionRequest, Transaction, Block, Receipt, Header>::block_by_hash(
                    rpc_client,
                    current_hash,
                    false,
                )
                .await?
                .ok_or_else(|| {
                    eyre::eyre!("Block with hash {} not found during fork validation", current_hash)
                })?;

                current_hash = block.header.parent_hash;
                current_number = block.header.number.saturating_sub(1);
            }

            // verify we reached the expected fork base
            if current_hash != fork_base_hash {
                return Err(eyre::eyre!(
                    "Fork validation failed: expected fork base hash {}, but found {} at block {}",
                    fork_base_hash,
                    current_hash,
                    current_number
                ));
            }

            debug!(
                "Fork validation successful: tip block {} is descendant of fork base {} ({})",
                current_block_info.number, self.fork_base_number, fork_base_hash
            );

            Ok(())
        })
    }
}
