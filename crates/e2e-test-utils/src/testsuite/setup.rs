//! Test setup utilities for configuring the initial state.

use crate::{
    testsuite::{Environment, NodeClient},
    NodeBuilderHelper,
};
use alloy_eips::BlockNumberOrTag;
use alloy_rpc_types_eth::{Block as RpcBlock, Header, Receipt, Transaction};
use eyre::{eyre, Result};
use reth_chainspec::ChainSpec;
use reth_node_builder::{NodeBuilder, NodeConfig, NodeHandle};
use reth_node_core::{
    args::{DiscoveryArgs, NetworkArgs},
    primitives::RecoveredBlock,
};
use reth_primitives::Block;
use reth_rpc_api::clients::EthApiClient;
use reth_tasks::TaskManager;
use revm::state::EvmState;
use std::{marker::PhantomData, sync::Arc};
use tokio::{
    sync::mpsc,
    time::{sleep, Duration},
};
use tracing::{debug, span, Level};

/// Configuration for setting upa test environment
#[derive(Debug)]
pub struct Setup<I> {
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
    /// Shutdown channel to stop nodes when setup is dropped
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Is this setup in dev mode
    pub is_dev: bool,
    /// Tracks instance generic.
    _phantom: PhantomData<I>,
}

impl<I> Default for Setup<I> {
    fn default() -> Self {
        Self {
            chain_spec: None,
            genesis: None,
            blocks: Vec::new(),
            state: None,
            network: NetworkSetup::default(),
            shutdown_tx: None,
            is_dev: true,
            _phantom: Default::default(),
        }
    }
}

impl<I> Drop for Setup<I> {
    fn drop(&mut self) {
        // Send shutdown signal if the channel exists
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
}

impl<I> Setup<I> {
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

    /// Set dev mode
    pub fn with_dev_mode(mut self, is_dev: bool) -> Self {
        self.is_dev = is_dev;
        self
    }

    /// Apply the setup to the environment
    pub async fn apply<N>(&mut self, env: &mut Environment<I>) -> Result<()>
    where
        N: NodeBuilderHelper,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        let node_clients = self.create_clients::<N>().await?;
        if node_clients.is_empty() {
            return Err(eyre!("No nodes were created"));
        }

        // wait for all nodes to be ready to accept RPC requests before proceeding
        for (idx, client) in node_clients.iter().enumerate() {
            let mut retry_count = 0;
            const MAX_RETRIES: usize = 5;
            let mut last_error = None;

            while retry_count < MAX_RETRIES {
                match EthApiClient::<Transaction, RpcBlock, Receipt, Header>::block_by_number(
                    &client.rpc,
                    BlockNumberOrTag::Latest,
                    false,
                )
                .await
                {
                    Ok(_) => {
                        debug!("Node {idx} RPC endpoint is ready");
                        break;
                    }
                    Err(e) => {
                        last_error = Some(e);
                        retry_count += 1;
                        debug!(
                            "Node {idx} RPC endpoint not ready, retry {retry_count}/{MAX_RETRIES}"
                        );
                        sleep(Duration::from_millis(500)).await;
                    }
                }
            }
            if retry_count == MAX_RETRIES {
                return Err(eyre!("Failed to connect to node {idx} RPC endpoint after {MAX_RETRIES} retries: {:?}", last_error));
            }
        }

        env.node_clients = node_clients;

        // TODO: For each block in self.blocks, replay it on the node

        Ok(())
    }

    /// Creates the given nodes
    async fn create_clients<N>(&self) -> eyre::Result<Vec<NodeClient>>
    where
        N: NodeBuilderHelper,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let is_dev = self.is_dev;
        let node_count = self.network.node_count;

        let tasks = TaskManager::current();
        let exec = tasks.executor();

        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };

        let mut clients = Vec::with_capacity(node_count);
        let mut nodes = Vec::with_capacity(node_count);

        for idx in 0..node_count {
            let node_config = NodeConfig::new(chain_spec.clone())
                .with_network(network_config.clone())
                .with_unused_ports()
                .with_rpc(
                    reth_node_core::args::RpcServerArgs::default().with_unused_ports().with_http(),
                )
                .set_dev(is_dev);

            let span = span!(Level::INFO, "node", idx);
            let _enter = span.enter();
            let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
                .testing_node(exec.clone())
                .node(Default::default())
                .launch()
                .await?;

            let rpc = node
                .rpc_server_handler()
                .http_client()
                .ok_or_else(|| eyre!("Failed to create HTTP RPC client for node"))?;
            let engine = node.auth_server_handle().http_client();

            clients.push(NodeClient { rpc, engine });
            nodes.push(node);
        }
        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        self.shutdown_tx = Some(shutdown_tx);

        // spawn a separate task just to handle the shutdown
        tokio::spawn(async move {
            // keep nodes and task manager in scope to ensure they're not dropped
            let _nodes = nodes;
            let _tasks = tasks;
            // Wait for shutdown signal
            let _ = shutdown_rx.recv().await;
            // nodes and task manager will be dropped here when the test completes
        });

        Ok(clients)
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
}

impl NetworkSetup {
    /// Create a new network setup with a single node
    pub fn single_node() -> Self {
        Self { node_count: 1 }
    }

    /// Create a new network setup with multiple nodes
    pub fn multi_node(count: usize) -> Self {
        Self { node_count: count }
    }
}
