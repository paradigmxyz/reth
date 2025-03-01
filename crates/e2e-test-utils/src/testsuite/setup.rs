//! Test setup utilities for configuring the initial state.

use crate::testsuite::Environment;
use eyre::Result;
use reth_chainspec::ChainSpec;
use reth_node_core::primitives::RecoveredBlock;
use reth_primitives::Block;
use revm::state::EvmState;
use std::sync::Arc;

/// Configuration for setting up a test environment
#[derive(Debug, Default)]
pub struct Setup {
    /// Chain specification to use
    pub chain_spec: Option<Arc<ChainSpec>>,
    /// Genesis block to use
    pub genesis: Option<Genesis>,
    /// Blocks to replay during setup
    pub blocks: Vec<RecoveredBlock<Block>>,
    /// Initial state to load
    pub state: Option<EvmState>,
    /// Network configuration
    pub network: NetworkSetup,
}

impl Setup {
    /// Create a new setup with default values
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the chain specification
    pub fn with_chain_spec(mut self, chain_spec: Arc<ChainSpec>) -> Self {
        self.chain_spec = Some(chain_spec);
        self
    }

    /// Set the genesis block
    pub fn with_genesis(mut self, genesis: Genesis) -> Self {
        self.genesis = Some(genesis);
        self
    }

    /// Add a block to replay during setup
    pub fn with_block(mut self, block: RecoveredBlock<Block>) -> Self {
        self.blocks.push(block);
        self
    }

    /// Add multiple blocks to replay during setup
    pub fn with_blocks(mut self, blocks: Vec<RecoveredBlock<Block>>) -> Self {
        self.blocks.extend(blocks);
        self
    }

    /// Set the initial state
    pub fn with_state(mut self, state: EvmState) -> Self {
        self.state = Some(state);
        self
    }

    /// Set the network configuration
    pub fn with_network(mut self, network: NetworkSetup) -> Self {
        self.network = network;
        self
    }

    /// Apply the setup to the environment
    pub async fn apply<I>(&self, _env: &mut Environment<I>) -> Result<()> {
        // Apply chain spec, genesis, blocks, state, and network configuration
        // This would involve setting up the node(s) with the specified configuration
        // and ensuring it's in the desired state before running tests

        // For each block in self.blocks, replay it on the node

        // Set up the network connections if multiple nodes

        Ok(())
    }
}

/// Genesis block configuration
#[derive(Debug)]
pub struct Genesis {}

/// Network configuration for setup
#[derive(Debug, Default)]
pub struct NetworkSetup {
    /// Number of nodes to create
    pub node_count: usize,
    /// Whether to disable discovery
    pub disable_discovery: bool,
}

impl NetworkSetup {
    /// Create a new network setup with a single node
    pub fn single_node() -> Self {
        Self { node_count: 1, disable_discovery: true }
    }

    /// Create a new network setup with multiple nodes
    pub fn multi_node(count: usize) -> Self {
        Self { node_count: count, disable_discovery: true }
    }
}
