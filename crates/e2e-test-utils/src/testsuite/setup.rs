//! Test setup utilities for configuring the initial state.

use crate::{
    setup_engine_with_connection, testsuite::Environment, NodeBuilderHelper,
    PayloadAttributesBuilder,
};
use alloy_eips::BlockNumberOrTag;
use alloy_primitives::B256;
use alloy_rpc_types_engine::{ForkchoiceState, PayloadAttributes};
use eyre::{eyre, Result};
use reth_chainspec::ChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_ethereum_primitives::Block;
use reth_network_p2p::sync::{NetworkSyncUpdater, SyncState};
use reth_node_api::{EngineTypes, NodeTypes, PayloadTypes, TreeConfig};
use reth_node_core::primitives::RecoveredBlock;
use reth_payload_builder::EthPayloadBuilderAttributes;
use revm::state::EvmState;
use std::{marker::PhantomData, path::Path, sync::Arc};
use tokio::{
    sync::mpsc,
    time::{sleep, Duration},
};
use tracing::debug;

/// Configuration for setting up test environment
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
    /// Holds the import result to keep nodes alive when using imported chain
    /// This is stored as an option to avoid lifetime issues with `tokio::spawn`
    import_result_holder: Option<crate::setup_import::ChainImportResult>,
    /// Path to RLP file to import during setup
    pub import_rlp_path: Option<std::path::PathBuf>,
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
            import_result_holder: None,
            import_rlp_path: None,
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

    /// Apply setup using pre-imported chain data from RLP file
    pub async fn apply_with_import<N>(
        &mut self,
        env: &mut Environment<I>,
        rlp_path: &Path,
    ) -> Result<()>
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
        >,
    {
        // Create nodes with imported chain data
        let import_result = self.create_nodes_with_import::<N>(rlp_path).await?;

        // Extract node clients
        let mut node_clients = Vec::new();
        let nodes = &import_result.nodes;
        for node in nodes {
            let rpc = node
                .rpc_client()
                .ok_or_else(|| eyre!("Failed to create HTTP RPC client for node"))?;
            let auth = node.auth_server_handle();
            let url = node.rpc_url();
            node_clients.push(crate::testsuite::NodeClient::new(rpc, auth, url));
        }

        // Store the import result to keep nodes alive
        // They will be dropped when the Setup is dropped
        self.import_result_holder = Some(import_result);

        // Finalize setup - this will wait for nodes and initialize states
        self.finalize_setup(env, node_clients, true).await
    }

    /// Apply the setup to the environment
    pub async fn apply<N>(&mut self, env: &mut Environment<I>) -> Result<()>
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
        >,
    {
        // If import_rlp_path is set, use apply_with_import instead
        if let Some(rlp_path) = self.import_rlp_path.take() {
            // Note: this future is quite large so we box it
            return Box::pin(self.apply_with_import::<N>(env, &rlp_path)).await;
        }
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let is_dev = self.is_dev;
        let node_count = self.network.node_count;

        let attributes_generator = self.create_attributes_generator::<N>();

        let result = setup_engine_with_connection::<N>(
            node_count,
            Arc::<N::ChainSpec>::new((*chain_spec).clone().into()),
            is_dev,
            self.tree_config.clone(),
            attributes_generator,
            self.network.connect_nodes,
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
                    let url = node.rpc_url();

                    node_clients.push(crate::testsuite::NodeClient::new(rpc, auth, url));
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
                return Err(eyre!("Failed to setup nodes: {}", e));
            }
        }

        // Finalize setup
        self.finalize_setup(env, node_clients, false).await
    }

    /// Create nodes with imported chain data
    ///
    /// Note: Currently this only supports `EthereumNode` due to the import process
    /// being Ethereum-specific. The generic parameter N is kept for consistency
    /// with other methods but is not used.
    async fn create_nodes_with_import<N>(
        &self,
        rlp_path: &Path,
    ) -> Result<crate::setup_import::ChainImportResult>
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
        >,
    {
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let attributes_generator = move |timestamp| {
            let attributes = PayloadAttributes {
                timestamp,
                prev_randao: B256::ZERO,
                suggested_fee_recipient: alloy_primitives::Address::ZERO,
                withdrawals: Some(vec![]),
                parent_beacon_block_root: Some(B256::ZERO),
            };
            EthPayloadBuilderAttributes::new(B256::ZERO, attributes)
        };

        crate::setup_import::setup_engine_with_chain_import(
            self.network.node_count,
            chain_spec,
            self.is_dev,
            self.tree_config.clone(),
            rlp_path,
            attributes_generator,
        )
        .await
    }

    /// Create the attributes generator function
    fn create_attributes_generator<N>(
        &self,
    ) -> impl Fn(u64) -> <<N as NodeTypes>::Payload as PayloadTypes>::PayloadBuilderAttributes + Copy
    where
        N: NodeBuilderHelper,
        LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
            <<N as NodeTypes>::Payload as PayloadTypes>::PayloadAttributes,
        >,
    {
        move |timestamp| {
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
        }
    }

    /// Common finalization logic for both apply methods
    async fn finalize_setup(
        &self,
        env: &mut Environment<I>,
        node_clients: Vec<crate::testsuite::NodeClient>,
        use_latest_block: bool,
    ) -> Result<()> {
        if node_clients.is_empty() {
            return Err(eyre!("No nodes were created"));
        }

        // Wait for all nodes to be ready
        self.wait_for_nodes_ready(&node_clients).await?;

        env.node_clients = node_clients;
        env.initialize_node_states(self.network.node_count);

        // Get initial block info (genesis or latest depending on use_latest_block)
        let (initial_block_info, genesis_block_info) = if use_latest_block {
            // For imported chain, get both latest and genesis
            let latest =
                self.get_block_info(&env.node_clients[0], BlockNumberOrTag::Latest).await?;
            let genesis =
                self.get_block_info(&env.node_clients[0], BlockNumberOrTag::Number(0)).await?;
            (latest, genesis)
        } else {
            // For fresh chain, both are genesis
            let genesis =
                self.get_block_info(&env.node_clients[0], BlockNumberOrTag::Number(0)).await?;
            (genesis, genesis)
        };

        // Initialize all node states
        for (node_idx, node_state) in env.node_states.iter_mut().enumerate() {
            node_state.current_block_info = Some(initial_block_info);
            node_state.latest_header_time = initial_block_info.timestamp;
            node_state.latest_fork_choice_state = ForkchoiceState {
                head_block_hash: initial_block_info.hash,
                safe_block_hash: initial_block_info.hash,
                finalized_block_hash: genesis_block_info.hash,
            };

            debug!(
                "Node {} initialized with block {} (hash: {})",
                node_idx, initial_block_info.number, initial_block_info.hash
            );
        }

        debug!(
            "Environment initialized with {} nodes, starting from block {} (hash: {})",
            self.network.node_count, initial_block_info.number, initial_block_info.hash
        );

        // In test environments, explicitly set sync state to Idle after initialization
        // This ensures that eth_syncing returns false as expected by tests
        if let Some(import_result) = &self.import_result_holder {
            for (idx, node_ctx) in import_result.nodes.iter().enumerate() {
                debug!("Setting sync state to Idle for node {}", idx);
                node_ctx.inner.network.update_sync_state(SyncState::Idle);
            }
        }

        Ok(())
    }

    /// Wait for all nodes to be ready to accept RPC requests
    async fn wait_for_nodes_ready(
        &self,
        node_clients: &[crate::testsuite::NodeClient],
    ) -> Result<()> {
        for (idx, client) in node_clients.iter().enumerate() {
            let mut retry_count = 0;
            const MAX_RETRIES: usize = 10;

            while retry_count < MAX_RETRIES {
                if client.is_ready().await {
                    debug!("Node {idx} RPC endpoint is ready");
                    break;
                }

                retry_count += 1;
                debug!("Node {idx} RPC endpoint not ready, retry {retry_count}/{MAX_RETRIES}");
                sleep(Duration::from_millis(500)).await;
            }

            if retry_count == MAX_RETRIES {
                return Err(eyre!(
                    "Failed to connect to node {idx} RPC endpoint after {MAX_RETRIES} retries"
                ));
            }
        }
        Ok(())
    }

    /// Get block info for a given block number or tag
    async fn get_block_info(
        &self,
        client: &crate::testsuite::NodeClient,
        block: BlockNumberOrTag,
    ) -> Result<crate::testsuite::BlockInfo> {
        let block = client
            .get_block_by_number(block)
            .await?
            .ok_or_else(|| eyre!("Block {:?} not found", block))?;

        Ok(crate::testsuite::BlockInfo {
            hash: block.header.hash,
            number: block.header.number,
            timestamp: block.header.timestamp,
        })
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
    /// Whether nodes should be connected to each other
    pub connect_nodes: bool,
}

impl NetworkSetup {
    /// Create a new network setup with a single node
    pub const fn single_node() -> Self {
        Self { node_count: 1, connect_nodes: true }
    }

    /// Create a new network setup with multiple nodes (connected)
    pub const fn multi_node(count: usize) -> Self {
        Self { node_count: count, connect_nodes: true }
    }

    /// Create a new network setup with multiple nodes (disconnected)
    pub const fn multi_node_unconnected(count: usize) -> Self {
        Self { node_count: count, connect_nodes: false }
    }
}
