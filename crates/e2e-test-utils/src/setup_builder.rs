//! Builder for configuring and creating test node setups.
//!
//! This module provides a flexible builder API for setting up test nodes with custom
//! configurations through closures that modify `NodeConfig` and `TreeConfig`.

use crate::{node::NodeTestContext, wallet::Wallet, NodeBuilderHelper, NodeHelperType, TmpDB};
use futures_util::future::TryJoinAll;
use reth_chainspec::EthChainSpec;
use reth_node_builder::{
    EngineNodeLauncher, NodeBuilder, NodeConfig, NodeHandle, NodeTypes, NodeTypesWithDBAdapter,
    PayloadTypes,
};
use reth_node_core::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
use reth_primitives_traits::AlloyBlockHeader;
use reth_provider::providers::BlockchainProvider;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::Runtime;
use std::sync::Arc;
use tracing::{span, Instrument, Level};

/// Type alias for tree config modifier closure
type TreeConfigModifier =
    Box<dyn Fn(reth_node_api::TreeConfig) -> reth_node_api::TreeConfig + Send + Sync>;

/// Type alias for node config modifier closure
type NodeConfigModifier<C> = Box<dyn Fn(NodeConfig<C>) -> NodeConfig<C> + Send + Sync>;

/// Builder for configuring and creating test node setups.
///
/// This builder allows customizing test node configurations through closures that
/// modify `NodeConfig` and `TreeConfig`. It avoids code duplication by centralizing
/// the node creation logic.
pub struct E2ETestSetupBuilder<N, F>
where
    N: NodeBuilderHelper,
    F: Fn(u64) -> <<N as NodeTypes>::Payload as PayloadTypes>::PayloadBuilderAttributes
        + Send
        + Sync
        + Copy
        + 'static,
{
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    attributes_generator: F,
    connect_nodes: bool,
    tree_config_modifier: Option<TreeConfigModifier>,
    node_config_modifier: Option<NodeConfigModifier<N::ChainSpec>>,
}

impl<N, F> E2ETestSetupBuilder<N, F>
where
    N: NodeBuilderHelper,
    F: Fn(u64) -> <<N as NodeTypes>::Payload as PayloadTypes>::PayloadBuilderAttributes
        + Send
        + Sync
        + Copy
        + 'static,
{
    /// Creates a new builder with the required parameters.
    pub fn new(num_nodes: usize, chain_spec: Arc<N::ChainSpec>, attributes_generator: F) -> Self {
        Self {
            num_nodes,
            chain_spec,
            attributes_generator,
            connect_nodes: true,
            tree_config_modifier: None,
            node_config_modifier: None,
        }
    }

    /// Sets whether nodes should be interconnected (default: true).
    pub const fn with_connect_nodes(mut self, connect_nodes: bool) -> Self {
        self.connect_nodes = connect_nodes;
        self
    }

    /// Sets a modifier function for the tree configuration.
    ///
    /// The closure receives the base tree config and returns a modified version.
    pub fn with_tree_config_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(reth_node_api::TreeConfig) -> reth_node_api::TreeConfig + Send + Sync + 'static,
    {
        self.tree_config_modifier = Some(Box::new(modifier));
        self
    }

    /// Sets a modifier function for the node configuration.
    ///
    /// The closure receives the base node config and returns a modified version.
    pub fn with_node_config_modifier<G>(mut self, modifier: G) -> Self
    where
        G: Fn(NodeConfig<N::ChainSpec>) -> NodeConfig<N::ChainSpec> + Send + Sync + 'static,
    {
        self.node_config_modifier = Some(Box::new(modifier));
        self
    }

    /// Enables v2 storage defaults (`--storage.v2`), routing tx hashes, history
    /// indices, etc. to `RocksDB` and changesets/senders to static files.
    pub fn with_storage_v2(self) -> Self {
        self.with_node_config_modifier(|mut config| {
            config.storage.v2 = true;
            config
        })
    }

    /// Builds and launches the test nodes.
    pub async fn build(
        self,
    ) -> eyre::Result<(
        Vec<NodeHelperType<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        Wallet,
    )> {
        let runtime = Runtime::test();

        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };

        // Apply tree config modifier if present, with test-appropriate defaults
        let base_tree_config =
            reth_node_api::TreeConfig::default().with_cross_block_cache_size(1024 * 1024);
        let tree_config = if let Some(modifier) = self.tree_config_modifier {
            modifier(base_tree_config)
        } else {
            base_tree_config
        };

        let mut nodes = (0..self.num_nodes)
            .map(async |idx| {
                // Create base node config
                let base_config = NodeConfig::new(self.chain_spec.clone())
                    .with_network(network_config.clone())
                    .with_unused_ports()
                    .with_rpc(
                        RpcServerArgs::default()
                            .with_unused_ports()
                            .with_http()
                            .with_http_api(RpcModuleSelection::All),
                    );

                // Apply node config modifier if present
                let node_config = if let Some(modifier) = &self.node_config_modifier {
                    modifier(base_config)
                } else {
                    base_config
                };

                let span = span!(Level::INFO, "node", idx);
                let node = N::default();
                let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
                    .testing_node(runtime.clone())
                    .with_types_and_provider::<N, BlockchainProvider<_>>()
                    .with_components(node.components_builder())
                    .with_add_ons(node.add_ons())
                    .launch_with_fn(|builder| {
                        let launcher = EngineNodeLauncher::new(
                            builder.task_executor().clone(),
                            builder.config().datadir(),
                            tree_config.clone(),
                        );
                        builder.launch_with(launcher)
                    })
                    .instrument(span)
                    .await?;

                let node = NodeTestContext::new(node, self.attributes_generator).await?;
                let genesis_number = self.chain_spec.genesis_header().number();
                let genesis = node.block_hash(genesis_number);
                node.update_forkchoice(genesis, genesis).await?;

                eyre::Ok(node)
            })
            .collect::<TryJoinAll<_>>()
            .await?;

        for idx in 0..self.num_nodes {
            let (prev, current) = nodes.split_at_mut(idx);
            let current = current.first_mut().unwrap();
            // Connect nodes if requested
            if self.connect_nodes {
                if let Some(prev_idx) = idx.checked_sub(1) {
                    prev[prev_idx].connect(current).await;
                }

                // Connect last node with the first if there are more than two
                if idx + 1 == self.num_nodes &&
                    self.num_nodes > 2 &&
                    let Some(first) = prev.first_mut()
                {
                    current.connect(first).await;
                }
            }
        }

        Ok((nodes, Wallet::default().with_chain_id(self.chain_spec.chain().into())))
    }
}

impl<N, F> std::fmt::Debug for E2ETestSetupBuilder<N, F>
where
    N: NodeBuilderHelper,
    F: Fn(u64) -> <<N as NodeTypes>::Payload as PayloadTypes>::PayloadBuilderAttributes
        + Send
        + Sync
        + Copy
        + 'static,
{
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("E2ETestSetupBuilder")
            .field("num_nodes", &self.num_nodes)
            .field("connect_nodes", &self.connect_nodes)
            .field("tree_config_modifier", &self.tree_config_modifier.as_ref().map(|_| "<closure>"))
            .field("node_config_modifier", &self.node_config_modifier.as_ref().map(|_| "<closure>"))
            .finish_non_exhaustive()
    }
}
