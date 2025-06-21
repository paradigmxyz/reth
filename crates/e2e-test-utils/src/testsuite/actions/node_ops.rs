//! Node-specific operations for multi-node testing.

use crate::testsuite::{Action, Environment};
use alloy_rpc_types_eth::{Block, Header, Receipt, Transaction};
use eyre::Result;
use futures_util::future::BoxFuture;
use reth_node_api::EngineTypes;
use reth_rpc_api::clients::EthApiClient;
use tracing::debug;

/// Action to select which node should be active for subsequent single-node operations.
#[derive(Debug)]
pub struct SelectActiveNode {
    /// Node index to set as active
    pub node_idx: usize,
}

impl SelectActiveNode {
    /// Create a new `SelectActiveNode` action
    pub const fn new(node_idx: usize) -> Self {
        Self { node_idx }
    }
}

impl<Engine> Action<Engine> for SelectActiveNode
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            env.set_active_node(self.node_idx)?;
            debug!("Set active node to {}", self.node_idx);
            Ok(())
        })
    }
}

/// Action to compare chain tips between two nodes.
#[derive(Debug)]
pub struct CompareNodeChainTips {
    /// First node index
    pub node_a: usize,
    /// Second node index
    pub node_b: usize,
    /// Whether tips should be the same or different
    pub should_be_equal: bool,
}

impl CompareNodeChainTips {
    /// Create a new action expecting nodes to have the same chain tip
    pub const fn expect_same(node_a: usize, node_b: usize) -> Self {
        Self { node_a, node_b, should_be_equal: true }
    }

    /// Create a new action expecting nodes to have different chain tips
    pub const fn expect_different(node_a: usize, node_b: usize) -> Self {
        Self { node_a, node_b, should_be_equal: false }
    }
}

impl<Engine> Action<Engine> for CompareNodeChainTips
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.node_a >= env.node_count() || self.node_b >= env.node_count() {
                return Err(eyre::eyre!("Node index out of bounds"));
            }

            let node_a_client = &env.node_clients[self.node_a];
            let node_b_client = &env.node_clients[self.node_b];

            // Get latest block from each node
            let block_a = EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                &node_a_client.rpc,
                alloy_eips::BlockNumberOrTag::Latest,
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to get latest block from node {}", self.node_a))?;

            let block_b = EthApiClient::<Transaction, Block, Receipt, Header>::block_by_number(
                &node_b_client.rpc,
                alloy_eips::BlockNumberOrTag::Latest,
                false,
            )
            .await?
            .ok_or_else(|| eyre::eyre!("Failed to get latest block from node {}", self.node_b))?;

            let tips_equal = block_a.header.hash == block_b.header.hash;

            debug!(
                "Node {} chain tip: {} (block {}), Node {} chain tip: {} (block {})",
                self.node_a,
                block_a.header.hash,
                block_a.header.number,
                self.node_b,
                block_b.header.hash,
                block_b.header.number
            );

            if self.should_be_equal && !tips_equal {
                return Err(eyre::eyre!(
                    "Expected nodes {} and {} to have the same chain tip, but node {} has {} and node {} has {}",
                    self.node_a, self.node_b, self.node_a, block_a.header.hash, self.node_b, block_b.header.hash
                ));
            }

            if !self.should_be_equal && tips_equal {
                return Err(eyre::eyre!(
                    "Expected nodes {} and {} to have different chain tips, but both have {}",
                    self.node_a,
                    self.node_b,
                    block_a.header.hash
                ));
            }

            Ok(())
        })
    }
}

/// Action to capture a block with a tag, associating it with a specific node.
#[derive(Debug)]
pub struct CaptureBlockOnNode {
    /// Tag name to associate with the block
    pub tag: String,
    /// Node index to capture the block from
    pub node_idx: usize,
}

impl CaptureBlockOnNode {
    /// Create a new `CaptureBlockOnNode` action
    pub fn new(tag: impl Into<String>, node_idx: usize) -> Self {
        Self { tag: tag.into(), node_idx }
    }
}

impl<Engine> Action<Engine> for CaptureBlockOnNode
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let node_state = env.node_state(self.node_idx)?;
            let current_block = node_state.current_block_info.ok_or_else(|| {
                eyre::eyre!("No current block information available for node {}", self.node_idx)
            })?;

            env.block_registry.insert(self.tag.clone(), (current_block, self.node_idx));

            debug!(
                "Captured block {} (hash: {}) from node {} with tag '{}'",
                current_block.number, current_block.hash, self.node_idx, self.tag
            );

            Ok(())
        })
    }
}

/// Action to get a block by tag and verify which node it came from.
#[derive(Debug)]
pub struct ValidateBlockTag {
    /// Tag to look up
    pub tag: String,
    /// Expected node index (optional)
    pub expected_node_idx: Option<usize>,
}

impl ValidateBlockTag {
    /// Create a new action to validate a block tag exists
    pub fn exists(tag: impl Into<String>) -> Self {
        Self { tag: tag.into(), expected_node_idx: None }
    }

    /// Create a new action to validate a block tag came from a specific node
    pub fn from_node(tag: impl Into<String>, node_idx: usize) -> Self {
        Self { tag: tag.into(), expected_node_idx: Some(node_idx) }
    }
}

impl<Engine> Action<Engine> for ValidateBlockTag
where
    Engine: EngineTypes,
{
    fn execute<'a>(&'a mut self, env: &'a mut Environment<Engine>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            let (block_info, node_idx) = env
                .block_registry
                .get(&self.tag)
                .copied()
                .ok_or_else(|| eyre::eyre!("Block tag '{}' not found in registry", self.tag))?;

            if let Some(expected_node) = self.expected_node_idx {
                if node_idx != expected_node {
                    return Err(eyre::eyre!(
                        "Block tag '{}' came from node {} but expected node {}",
                        self.tag,
                        node_idx,
                        expected_node
                    ));
                }
            }

            debug!(
                "Validated block tag '{}': block {} (hash: {}) from node {}",
                self.tag, block_info.number, block_info.hash, node_idx
            );

            Ok(())
        })
    }
}
