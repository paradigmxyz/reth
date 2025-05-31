//! Test setup utilities for configuring the initial state.

use crate::{setup_engine, testsuite::Environment, NodeBuilderHelper, PayloadAttributesBuilder};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use alloy_rpc_types_eth::{Block as RpcBlock, Header, Receipt, Transaction};
use eyre::{eyre, Result};
use reth_chainspec::ChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_ethereum_primitives::Block;
use reth_node_api::{EngineTypes, NodeTypes, PayloadTypes, TreeConfig};
use reth_node_core::primitives::RecoveredBlock;
use reth_payload_builder::EthPayloadBuilderAttributes;
use reth_rpc_api::clients::EthApiClient;
use revm::state::EvmState;
use std::{marker::PhantomData, sync::Arc};
use tokio::{
    sync::mpsc,
    time::{sleep, Duration},
};
use tracing::{debug, error};

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
    /// Engine tree configuration
    pub tree_config: TreeConfig,
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
            tree_config: TreeConfig::default(),
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

impl<I> Setup<I>
where
    I: EngineTypes,
{
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
    pub const fn with_genesis(mut self, genesis: Genesis) -> Self {
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
    pub const fn with_network(mut self, network: NetworkSetup) -> Self {
        self.network = network;
        self
    }

    /// Set dev mode
    pub const fn with_dev_mode(mut self, is_dev: bool) -> Self {
        self.is_dev = is_dev;
        self
    }

    /// Set the engine tree configuration
    pub const fn with_tree_config(mut self, tree_config: TreeConfig) -> Self {
        self.tree_config = tree_config;
        self
    }

    /// Apply the setup to the environment
    pub async fn apply<N>(&mut self, env: &mut Environment<I>) -> Result<()>
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
        >,
    {
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);

        self.shutdown_tx = Some(shutdown_tx);

        let is_dev = self.is_dev;
        let node_count = self.network.node_count;

        let attributes_generator = move |timestamp| {
            let attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: alloy_primitives::Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadBuilderAttributes::from(
                EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
            )
        };

        let result = setup_engine::<N>(
            node_count,
            Arc::<N::ChainSpec>::new((*chain_spec).clone().into()),
            is_dev,
            self.tree_config.clone(),
            attributes_generator,
        )
        .await;

        let mut node_clients = Vec::new();
        match result {
            Ok((nodes, executor, _wallet)) => {
                // create HTTP clients for each node's RPC and Engine API endpoints
                for node in &nodes {
                    let rpc = node
                        .rpc_client()
                        .ok_or_else(|| eyre!("Failed to create HTTP RPC client for node"))?;
                    let auth = node.auth_server_handle();

                    node_clients.push(crate::testsuite::NodeClient::new(rpc, auth));
                }

                // spawn a separate task just to handle the shutdown
                tokio::spawn(async move {
                    // keep nodes and executor in scope to ensure they're not dropped
                    let _nodes = nodes;
                    let _executor = executor;
                    // Wait for shutdown signal
                    let _ = shutdown_rx.recv().await;
                    // nodes and executor will be dropped here when the test completes
                });
            }
            Err(e) => {
                error!("Failed to setup nodes: {}", e);
                return Err(eyre!("Failed to setup nodes: {}", e));
            }
        }

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

        // Initialize the environment with genesis block information
        let first_client = &env.node_clients[0];
        let genesis_block =
            EthApiClient::<Transaction, RpcBlock, Receipt, Header>::block_by_number(
                &first_client.rpc,
                BlockNumberOrTag::Number(0),
                false,
            )
            .await?
            .ok_or_else(|| eyre!("Genesis block not found"))?;

        env.latest_block_info = Some(crate::testsuite::LatestBlockInfo {
            hash: genesis_block.header.hash,
            number: genesis_block.header.number,
        });

        env.latest_header_time = genesis_block.header.timestamp;
        env.latest_fork_choice_state = ForkchoiceState {
            head_block_hash: genesis_block.header.hash,
            safe_block_hash: genesis_block.header.hash,
            finalized_block_hash: genesis_block.header.hash,
        };

        debug!(
            "Environment initialized with genesis block {} (hash: {})",
            genesis_block.header.number, genesis_block.header.hash
        );

        // TODO: For each block in self.blocks, replay it on the node

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
}

impl NetworkSetup {
    /// Create a new network setup with a single node
    pub const fn single_node() -> Self {
        Self { node_count: 1 }
    }

    /// Create a new network setup with multiple nodes
    pub const fn multi_node(count: usize) -> Self {
        Self { node_count: count }
    }
}
