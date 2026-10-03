//! Test setup utilities for configuring the initial state.

use crate::{
    testsuite::{actions::expect_fcu_valid, Environment},
    wait::{poll_until, poll_until_with, PollOpts},
    E2ETestSetupExt, NodeBuilderHelper,
};
use alloy_eips::BlockNumberOrTag;
use alloy_rpc_types_engine::ForkchoiceState;
use eyre::{eyre, Result};
use reth_chainspec::ChainSpec;
use reth_node_api::{EngineTypes, PayloadTypes, TreeConfig};
use reth_node_core::args::StorageArgs;
use reth_rpc_api::clients::EngineApiClient;
use std::{marker::PhantomData, path::Path, sync::Arc, time::Duration};
use tokio::sync::mpsc;
use tracing::debug;

/// Configuration for setting up test environment
#[derive(Debug)]
pub struct Setup<I> {
    /// Chain specification to use
    pub chain_spec: Option<Arc<ChainSpec>>,
    /// Network configuration
    pub network: NetworkSetup,
    /// Engine tree configuration
    pub tree_config: TreeConfig,
    /// Shutdown channel to stop nodes when setup is dropped
    shutdown_tx: Option<mpsc::Sender<()>>,
    /// Is this setup in dev mode
    pub is_dev: bool,
    /// Whether to use v2 storage mode (hashed keys, static file changesets, rocksdb history).
    ///
    /// Defaults to the node's `--storage.v2` default.
    pub storage_v2: bool,
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
            network: NetworkSetup::default(),
            tree_config: TreeConfig::default(),
            shutdown_tx: None,
            is_dev: true,
            storage_v2: StorageArgs::default().v2,
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
    /// Set the chain specification
    pub fn with_chain_spec(mut self, chain_spec: Arc<ChainSpec>) -> Self {
        self.chain_spec = Some(chain_spec);
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

    /// Set whether to use v2 storage mode (hashed keys, static file changesets, rocksdb history)
    pub const fn with_storage_v2(mut self, storage_v2: bool) -> Self {
        self.storage_v2 = storage_v2;
        self
    }

    /// Apply setup using pre-imported chain data from RLP file
    ///
    /// Returns once the engine of every node accepted the latest imported block as head and safe
    /// block, with genesis as finalized block, which is the forkchoice state recorded in `env`.
    /// Returns an error if a node does not accept it within [`WAIT_TIMEOUT`].
    ///
    /// [`WAIT_TIMEOUT`]: crate::wait::WAIT_TIMEOUT
    pub async fn apply_with_import<N>(
        &mut self,
        env: &mut Environment<I>,
        rlp_path: &Path,
    ) -> Result<()>
    where
        N: NodeBuilderHelper<Payload = I>,
    {
        // Note: this future is quite large so we box it
        Box::pin(self.apply_with_import_(env, rlp_path)).await
    }

    /// Apply setup using pre-imported chain data from RLP file
    async fn apply_with_import_(
        &mut self,
        env: &mut Environment<I>,
        rlp_path: &Path,
    ) -> Result<()> {
        // Create nodes with imported chain data
        let import_result = self.create_nodes_with_import(rlp_path).await?;

        // Extract node clients
        let mut node_clients = Vec::new();
        let nodes = &import_result.nodes;
        for node in nodes {
            let rpc = node
                .rpc_client()
                .ok_or_else(|| eyre!("Failed to create HTTP RPC client for node"))?;
            let auth = node.auth_server_handle();
            let url = node.rpc_url();
            // TODO: Pass beacon_engine_handle once import system supports generic types
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
        N: NodeBuilderHelper<Payload = I, ChainSpec: From<ChainSpec>>,
    {
        // Note: this future is quite large so we box it
        Box::pin(self.apply_::<N>(env)).await
    }

    /// Apply the setup to the environment
    async fn apply_<N>(&mut self, env: &mut Environment<I>) -> Result<()>
    where
        N: NodeBuilderHelper<Payload = I, ChainSpec: From<ChainSpec>>,
    {
        // If import_rlp_path is set, use apply_with_import instead
        if let Some(rlp_path) = self.import_rlp_path.take() {
            return self.apply_with_import::<N>(env, &rlp_path).await;
        }
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        let (shutdown_tx, mut shutdown_rx) = mpsc::channel(1);
        self.shutdown_tx = Some(shutdown_tx);

        let tree_config = self.tree_config.clone();

        let result = N::test_setup(
            self.network.node_count,
            Arc::<N::ChainSpec>::new((*chain_spec).clone().into()),
        )
        .with_tree_config_modifier(move |base| {
            tree_config.clone().with_cross_block_cache_size(base.cross_block_cache_size())
        })
        .with_dev_mode(self.is_dev)
        .with_storage_v2(self.storage_v2)
        .with_connect_nodes(self.network.connect_nodes)
        .build()
        .await;

        let mut node_clients = Vec::new();
        match result {
            Ok((nodes, _wallet)) => {
                // create HTTP clients for each node's RPC and Engine API endpoints
                for node in &nodes {
                    node_clients.push(node.to_node_client()?);
                }

                // spawn a separate task just to handle the shutdown
                tokio::spawn(async move {
                    // keep nodes in scope to ensure they're not dropped
                    let _nodes = nodes;
                    // Wait for shutdown signal
                    let _ = shutdown_rx.recv().await;
                    // nodes will be dropped here when the test completes
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
    async fn create_nodes_with_import(
        &self,
        rlp_path: &Path,
    ) -> Result<crate::setup_import::ChainImportResult> {
        let chain_spec =
            self.chain_spec.clone().ok_or_else(|| eyre!("Chain specification is required"))?;

        crate::setup_import::setup_engine_with_chain_import(
            self.network.node_count,
            chain_spec,
            self.is_dev,
            self.storage_v2,
            self.tree_config.clone(),
            rlp_path,
        )
        .await
    }

    /// Common finalization logic for both apply methods
    async fn finalize_setup(
        &self,
        env: &mut Environment<I>,
        node_clients: Vec<crate::testsuite::NodeClient<I>>,
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
        let fork_choice_state = ForkchoiceState {
            head_block_hash: initial_block_info.hash,
            safe_block_hash: initial_block_info.hash,
            finalized_block_hash: genesis_block_info.hash,
        };
        for (node_idx, node_state) in env.node_states.iter_mut().enumerate() {
            node_state.current_block_info = Some(initial_block_info);
            node_state.latest_header_time = initial_block_info.timestamp;
            node_state.latest_fork_choice_state = fork_choice_state;

            debug!(
                "Node {} initialized with block {} (hash: {})",
                node_idx, initial_block_info.number, initial_block_info.hash
            );
        }

        // Fresh nodes are launched with genesis as their forkchoice state, nodes on an imported
        // chain are not, so make the imported head canonical before actions build on it.
        if use_latest_block {
            self.wait_for_forkchoice_valid(&env.node_clients, fork_choice_state).await?;
        }

        debug!(
            "Environment initialized with {} nodes, starting from block {} (hash: {})",
            self.network.node_count, initial_block_info.number, initial_block_info.hash
        );

        Ok(())
    }

    /// Wait for all nodes to be ready to accept RPC requests
    async fn wait_for_nodes_ready<P>(
        &self,
        node_clients: &[crate::testsuite::NodeClient<P>],
    ) -> Result<()>
    where
        P: PayloadTypes,
    {
        for (idx, client) in node_clients.iter().enumerate() {
            poll_until(format!("node {idx} RPC endpoint to accept requests"), || async {
                Ok(client.is_ready().await.then_some(()))
            })
            .await?;
            debug!("Node {idx} RPC endpoint is ready");
        }
        Ok(())
    }

    /// Waits until the engine of every node accepts `state` as its forkchoice state.
    ///
    /// The chain import leaves some stage checkpoints, e.g. of the prune stages, behind the
    /// imported head, so a node launched on an imported chain starts with a backfill run to the
    /// head and answers forkchoice updates with SYNCING until it finished. This resends the update
    /// until the node answers with another status, and returns an error unless that status is
    /// VALID. Every attempt is a forkchoice update, so attempts are spaced further apart than the
    /// default poll interval.
    async fn wait_for_forkchoice_valid(
        &self,
        node_clients: &[crate::testsuite::NodeClient<I>],
        state: ForkchoiceState,
    ) -> Result<()> {
        for (idx, client) in node_clients.iter().enumerate() {
            let engine = client.engine.http_client();
            let response = poll_until_with(
                PollOpts { interval: Duration::from_millis(100), ..Default::default() },
                format!("node {idx} to stop syncing to block {}", state.head_block_hash),
                || async {
                    let response =
                        EngineApiClient::<I>::fork_choice_updated_v3(&engine, state, None).await?;
                    Ok((!response.is_syncing()).then_some(response))
                },
            )
            .await?;
            expect_fcu_valid(
                &response,
                &format!("Node {idx} forkchoice update to block {}", state.head_block_hash),
            )?;
        }
        Ok(())
    }

    /// Get block info for a given block number or tag
    async fn get_block_info<P>(
        &self,
        client: &crate::testsuite::NodeClient<P>,
        block: BlockNumberOrTag,
    ) -> Result<crate::testsuite::BlockInfo>
    where
        P: PayloadTypes,
    {
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
