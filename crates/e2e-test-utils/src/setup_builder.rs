//! Builder for configuring and creating test node setups.
//!
//! This module provides a flexible builder API for setting up test nodes with custom
//! configurations through closures that modify `NodeConfig` and `TreeConfig`.

use crate::{node::NodeTestContext, wallet::Wallet, NodeBuilderHelper, NodeHelperType, TmpDB};
use reth_chainspec::EthChainSpec;
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_node_builder::{
    EngineNodeLauncher, NodeBuilder, NodeConfig, NodeHandle, NodeTypes, NodeTypesWithDBAdapter,
    PayloadAttributesBuilder, PayloadTypes,
};
use reth_node_core::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
use reth_provider::providers::BlockchainProvider;
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::TaskManager;
use std::sync::Arc;
use tracing::{span, Level};

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
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Payload as PayloadTypes>::PayloadAttributes>,
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
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Payload as PayloadTypes>::PayloadAttributes>,
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

    /// Builds and launches the test nodes.
    pub async fn build(
        self,
    ) -> eyre::Result<(
        Vec<NodeHelperType<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
        TaskManager,
        Wallet,
    )> {
        let tasks = TaskManager::current();
        let exec = tasks.executor();

        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };

        // Apply tree config modifier if present
        let tree_config = if let Some(modifier) = self.tree_config_modifier {
            modifier(reth_node_api::TreeConfig::default())
        } else {
            reth_node_api::TreeConfig::default()
        };

        let mut nodes: Vec<NodeTestContext<_, _>> = Vec::with_capacity(self.num_nodes);

        for idx in 0..self.num_nodes {
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
            let _enter = span.enter();
            let node = N::default();
            let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config)
                .testing_node(exec.clone())
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
                .await?;

            let mut node = NodeTestContext::new(node, self.attributes_generator).await?;

            let genesis = node.block_hash(0);
            node.update_forkchoice(genesis, genesis).await?;

            // Connect nodes if requested
            if self.connect_nodes {
                if let Some(previous_node) = nodes.last_mut() {
                    previous_node.connect(&mut node).await;
                }

                // Connect last node with the first if there are more than two
                if idx + 1 == self.num_nodes &&
                    self.num_nodes > 2 &&
                    let Some(first_node) = nodes.first_mut()
                {
                    node.connect(first_node).await;
                }
            }

            nodes.push(node);
        }

        Ok((nodes, tasks, Wallet::default().with_chain_id(self.chain_spec.chain().into())))
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
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Payload as PayloadTypes>::PayloadAttributes>,
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
