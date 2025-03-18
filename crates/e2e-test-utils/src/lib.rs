//! Utilities for end-to-end tests.

use node::NodeTestContext;
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_engine_local::LocalPayloadAttributesBuilder;
use reth_network_api::test_utils::PeersHandleProvider;
use reth_node_builder::{
    components::NodeComponentsBuilder,
    rpc::{EngineValidatorAddOn, RethRpcAddOns},
    EngineNodeLauncher, FullNodeTypesAdapter, Node, NodeAdapter, NodeBuilder, NodeComponents,
    NodeConfig, NodeHandle, NodePrimitives, NodeTypesWithDBAdapter, NodeTypesWithEngine,
    PayloadAttributesBuilder, PayloadTypes,
};
use reth_node_core::args::{DiscoveryArgs, NetworkArgs, RpcServerArgs};
use reth_provider::providers::{BlockchainProvider, NodeTypesForProvider};
use reth_rpc_server_types::RpcModuleSelection;
use reth_tasks::TaskManager;
use std::sync::Arc;
use tracing::{span, Level};
use wallet::Wallet;

/// Wrapper type to create test nodes
pub mod node;
pub mod testsuite;

/// Helper for transaction operations
pub mod transaction;

/// Helper type to yield accounts from mnemonic
pub mod wallet;

/// Helper for payload operations
mod payload;

/// Helper for network operations
mod network;

/// Helper for rpc operations
mod rpc;

/// Creates the initial setup with `num_nodes` started and interconnected.
pub async fn setup<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
    attributes_generator: impl Fn(u64) -> <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes + Send + Sync + Copy + 'static,
) -> eyre::Result<(Vec<NodeHelperType<N>>, TaskManager, Wallet)>
where
    N: Default + Node<TmpNodeAdapter<N>> + NodeTypesForProvider + NodeTypesWithEngine,
    N::ComponentsBuilder: NodeComponentsBuilder<
        TmpNodeAdapter<N>,
        Components: NodeComponents<TmpNodeAdapter<N>, Network: PeersHandleProvider>,
    >,
    N::AddOns: RethRpcAddOns<Adapter<N>> + EngineValidatorAddOn<Adapter<N>>,
    LocalPayloadAttributesBuilder<N::ChainSpec>: PayloadAttributesBuilder<
        <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadAttributes,
    >,
{
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
        ..NetworkArgs::default()
    };

    // Create nodes and peer them
    let mut nodes: Vec<NodeTestContext<_, _>> = Vec::with_capacity(num_nodes);

    for idx in 0..num_nodes {
        let node_config = NodeConfig::new(chain_spec.clone())
            .with_network(network_config.clone())
            .with_unused_ports()
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http())
            .set_dev(is_dev);

        let span = span!(Level::INFO, "node", idx);
        let _enter = span.enter();
        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
            .testing_node(exec.clone())
            .node(Default::default())
            .launch()
            .await?;

        let mut node = NodeTestContext::new(node, attributes_generator).await?;

        // Connect each node in a chain.
        if let Some(previous_node) = nodes.last_mut() {
            previous_node.connect(&mut node).await;
        }

        // Connect last node with the first if there are more than two
        if idx + 1 == num_nodes && num_nodes > 2 {
            if let Some(first_node) = nodes.first_mut() {
                node.connect(first_node).await;
            }
        }

        nodes.push(node);
    }

    Ok((nodes, tasks, Wallet::default().with_chain_id(chain_spec.chain().into())))
}

/// Creates the initial setup with `num_nodes` started and interconnected.
pub async fn setup_engine<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
    attributes_generator: impl Fn(u64) -> <<N as NodeTypesWithEngine>::Engine as PayloadTypes>::PayloadBuilderAttributes + Send + Sync + Copy + 'static,
) -> eyre::Result<(
    Vec<NodeHelperType<N, BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>>>,
    TaskManager,
    Wallet,
)>
where
    N: NodeBuilderHelper,
    LocalPayloadAttributesBuilder<N::ChainSpec>:
        PayloadAttributesBuilder<<N::Engine as PayloadTypes>::PayloadAttributes>,
{
    let tasks = TaskManager::current();
    let exec = tasks.executor();

    let network_config = NetworkArgs {
        discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
        ..NetworkArgs::default()
    };

    // Create nodes and peer them
    let mut nodes: Vec<NodeTestContext<_, _>> = Vec::with_capacity(num_nodes);

    for idx in 0..num_nodes {
        let node_config = NodeConfig::new(chain_spec.clone())
            .with_network(network_config.clone())
            .with_unused_ports()
            .with_rpc(
                RpcServerArgs::default()
                    .with_unused_ports()
                    .with_http()
                    .with_http_api(RpcModuleSelection::All),
            )
            .set_dev(is_dev);

        let span = span!(Level::INFO, "node", idx);
        let _enter = span.enter();
        let node = N::default();
        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
            .testing_node(exec.clone())
            .with_types_and_provider::<N, BlockchainProvider<_>>()
            .with_components(node.components_builder())
            .with_add_ons(node.add_ons())
            .launch_with_fn(|builder| {
                let launcher = EngineNodeLauncher::new(
                    builder.task_executor().clone(),
                    builder.config().datadir(),
                    Default::default(),
                );
                builder.launch_with(launcher)
            })
            .await?;

        let mut node = NodeTestContext::new(node, attributes_generator).await?;

        let genesis = node.block_hash(0);
        node.update_forkchoice(genesis, genesis).await?;

        // Connect each node in a chain.
        if let Some(previous_node) = nodes.last_mut() {
            previous_node.connect(&mut node).await;
        }

        // Connect last node with the first if there are more than two
        if idx + 1 == num_nodes && num_nodes > 2 {
            if let Some(first_node) = nodes.first_mut() {
                node.connect(first_node).await;
            }
        }

        nodes.push(node);
    }

    Ok((nodes, tasks, Wallet::default().with_chain_id(chain_spec.chain().into())))
}

// Type aliases

/// Testing database
pub type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
type TmpNodeAdapter<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> =
    FullNodeTypesAdapter<N, TmpDB, Provider>;

/// Type alias for a `NodeAdapter`
pub type Adapter<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> = NodeAdapter<
    TmpNodeAdapter<N, Provider>,
    <<N as Node<TmpNodeAdapter<N, Provider>>>::ComponentsBuilder as NodeComponentsBuilder<
        TmpNodeAdapter<N, Provider>,
    >>::Components,
>;

/// Type alias for a type of `NodeHelper`
pub type NodeHelperType<N, Provider = BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>> =
    NodeTestContext<Adapter<N, Provider>, <N as Node<TmpNodeAdapter<N, Provider>>>::AddOns>;

/// Helper trait to simplify bounds when calling setup functions.
pub trait NodeBuilderHelper
where
    Self: Default
        + NodeTypesForProvider
        + NodeTypesWithEngine<
            Engine: PayloadTypes<
                PayloadBuilderAttributes: From<reth_payload_builder::EthPayloadBuilderAttributes>,
            >,
        > + Node<
            TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            Primitives: NodePrimitives<
                BlockHeader = alloy_consensus::Header,
                BlockBody = alloy_consensus::BlockBody<
                    <Self::Primitives as NodePrimitives>::SignedTx,
                >,
            >,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                Components: NodeComponents<
                    TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                    Network: PeersHandleProvider,
                >,
            >,
            AddOns: RethRpcAddOns<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            > + EngineValidatorAddOn<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            >,
            ChainSpec: From<ChainSpec> + Clone,
        >,
    LocalPayloadAttributesBuilder<Self::ChainSpec>:
        PayloadAttributesBuilder<<Self::Engine as PayloadTypes>::PayloadAttributes>,
{
}

impl<T> NodeBuilderHelper for T
where
    Self: Default
        + NodeTypesForProvider
        + NodeTypesWithEngine<
            Engine: PayloadTypes<
                PayloadBuilderAttributes: From<reth_payload_builder::EthPayloadBuilderAttributes>,
            >,
        > + Node<
            TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            Primitives: NodePrimitives<
                BlockHeader = alloy_consensus::Header,
                BlockBody = alloy_consensus::BlockBody<
                    <Self::Primitives as NodePrimitives>::SignedTx,
                >,
            >,
            ComponentsBuilder: NodeComponentsBuilder<
                TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                Components: NodeComponents<
                    TmpNodeAdapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
                    Network: PeersHandleProvider,
                >,
            >,
            AddOns: RethRpcAddOns<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            > + EngineValidatorAddOn<
                Adapter<Self, BlockchainProvider<NodeTypesWithDBAdapter<Self, TmpDB>>>,
            >,
            ChainSpec: From<ChainSpec> + Clone,
        >,
    LocalPayloadAttributesBuilder<Self::ChainSpec>:
        PayloadAttributesBuilder<<Self::Engine as PayloadTypes>::PayloadAttributes>,
{
}
