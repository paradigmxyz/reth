//! Test setup utilities for configuring the initial state.

use crate::{setup_engine, testsuite::Environment, Adapter, TmpDB, TmpNodeAdapter};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_rpc_types_engine::PayloadAttributes;
use alloy_rpc_types_eth::{Block as RpcBlock, Header, Receipt, Transaction};
use eyre::{eyre, Result};
use jsonrpsee::http_client::HttpClientBuilder;
use reth_chainspec::ChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_api::NodePrimitives;
use reth_node_builder::{
    components::NodeComponentsBuilder,
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    Node, NodeComponents, NodeTypesWithDBAdapter, NodeTypesWithEngine, PayloadAttributesBuilder,
    PayloadTypes,
};
use reth_node_core::primitives::RecoveredBlock;
use reth_primitives::Block;
use reth_provider::providers::{BlockchainProvider, NodeTypesForProvider};
use reth_rpc_api::clients::EthApiClient;
use revm::state::EvmState;
use std::sync::Arc;
use tokio::{
    sync::{mpsc, oneshot},
    time::{sleep, Duration},
};
use tracing::{debug, error};

/// Configuration for setting upa test environment
#[derive(Debug)]
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
    /// Shutdown channel to stop nodes when setup is dropped
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Is this setup in dev mode
    pub is_dev: bool,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            chain_spec: None,
            genesis: None,
            blocks: Vec::new(),
            state: None,
            network: NetworkSetup::default(),
            shutdown_tx: None,
            is_dev: true,
        }
    }
}

impl Drop for Setup {
    fn drop(&mut self) {
        // Send shutdown signal if the channel exists
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.try_send(());
        }
    }
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

    /// Set dev mode
    pub fn with_dev_mode(mut self, is_dev: bool) -> Self {
        self.is_dev = is_dev;
        self
    }

    /// Apply the setup to the environment
    pub async fn apply<N>(&mut self, env: &mut Environment) -> Result<()>
    where
        N: Default
            + Node<TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>
            + NodeTypesWithEngine
            + NodeTypesForProvider,
        N::Primitives: NodePrimitives<
            BlockHeader = alloy_consensus::Header,
            BlockBody = alloy_consensus::BlockBody<<N::Primitives as NodePrimitives>::SignedTx>,
        >,
        N::ComponentsBuilder: NodeComponentsBuilder<
            TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
            Components: NodeComponents<
                TmpNodeAdapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>,
                Network: PeersHandleProvider,
            >,
        >,
        N::AddOns: RethRpcAddOns<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>
            + EngineValidatorAddOn<Adapter<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadAttributes,
        >,
        <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes:
            From<reth_payload_builder::EthPayloadBuilderAttributes>,
        N::ChainSpec: From<ChainSpec> + Clone,
    {
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let (clients_tx, clients_rx) = oneshot::channel();
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
            <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes::from(
                reth_payload_builder::EthPayloadBuilderAttributes::new(B256::ZERO, attributes),
            )
        };

        let result = setup_engine::<N>(
            node_count,
            Arc::<N::ChainSpec>::new((*chain_spec).clone().into()),
            is_dev,
            attributes_generator,
        )
        .await;

        match result {
            Ok((nodes, _tasks, _wallet)) => {
                // create HTTP clients for each node's RPC and Engine API endpoints
                let mut node_clients = Vec::with_capacity(nodes.len());
                for node in &nodes {
                    let rpc_url = node.rpc_url();
                    let engine_url = node.engine_api_url();
                    debug!("rpc_url: {rpc_url}, engine_url: {engine_url}");

                    let rpc_client = match HttpClientBuilder::default().build(&rpc_url) {
                        Ok(client) => client,
                        Err(e) => {
                            error!("Failed to create RPC client for {}: {}", rpc_url, e);
                            let _ = clients_tx.send(Vec::new());
                            return Err(eyre!("Failed to create RPC client: {}", e));
                        }
                    };

                    let engine_client = match HttpClientBuilder::default().build(&engine_url) {
                        Ok(client) => client,
                        Err(e) => {
                            error!("Failed to create Engine API client for {}: {}", engine_url, e);
                            let _ = clients_tx.send(Vec::new());
                            return Err(eyre!("Failed to create Engine API client: {}", e));
                        }
                    };

                    node_clients.push(crate::testsuite::NodeClient {
                        rpc: rpc_client,
                        engine: engine_client,
                    });
                }

                // send the clients back through the channel
                let _ = clients_tx.send(node_clients);

                // spawn a separate task just to handle the shutdown
                tokio::spawn(async move {
                    // keep nodes in scope to ensure they're not dropped
                    let _nodes = nodes;
                    // Wait for shutdown signal
                    let _ = shutdown_rx.recv().await;
                    // nodes will be dropped here when the task completes
                });
            }
            Err(e) => {
                let _ = clients_tx.send(Vec::new());
                error!("Failed to setup nodes: {}", e);
                return Err(eyre!("Failed to setup nodes: {}", e));
            }
        }

        // wait for the clients to be created
        let node_clients =
            clients_rx.await.map_err(|_| eyre!("Failed to receive clients from setup task"))?;

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
