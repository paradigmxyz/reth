//! Utilities for end-to-end tests.

use std::sync::Arc;

use node::NodeTestContext;
use reth::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    network::PeersHandleProvider,
    rpc::api::eth::{helpers::AddDevSigners, FullEthApiServer},
    tasks::TaskManager,
};
use reth_chainspec::{EthChainSpec, EthereumHardforks};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_node_builder::{
    components::NodeComponentsBuilder, rpc::EthApiBuilderProvider, FullNodeTypesAdapter, Node,
    NodeAdapter, NodeAddOns, NodeComponents, NodeTypesWithDBAdapter, NodeTypesWithEngine,
    RethFullAdapter,
};
use reth_provider::providers::BlockchainProvider;
use tracing::{span, Level};
use wallet::Wallet;

/// Wrapper type to create test nodes
pub mod node;

/// Helper for transaction operations
pub mod transaction;

/// Helper type to yield accounts from mnemonic
pub mod wallet;

/// Helper for payload operations
mod payload;

/// Helper for network operations
mod network;

/// Helper for engine api operations
mod engine_api;
/// Helper for rpc operations
mod rpc;

/// Helper traits
mod traits;

/// Creates the initial setup with `num_nodes` started and interconnected.
pub async fn setup<N>(
    num_nodes: usize,
    chain_spec: Arc<N::ChainSpec>,
    is_dev: bool,
) -> eyre::Result<(Vec<NodeHelperType<N, N::AddOns>>, TaskManager, Wallet)>
where
    N: Default + Node<TmpNodeAdapter<N>> + NodeTypesWithEngine<ChainSpec: EthereumHardforks>,
    N::ComponentsBuilder: NodeComponentsBuilder<
        TmpNodeAdapter<N>,
        Components: NodeComponents<TmpNodeAdapter<N>, Network: PeersHandleProvider>,
    >,
    N::AddOns: NodeAddOns<
        Adapter<N>,
        EthApi: FullEthApiServer + AddDevSigners + EthApiBuilderProvider<Adapter<N>>,
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

        let mut node = NodeTestContext::new(node).await?;

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

type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
type TmpNodeAdapter<N> = FullNodeTypesAdapter<
    NodeTypesWithDBAdapter<N, TmpDB>,
    BlockchainProvider<NodeTypesWithDBAdapter<N, TmpDB>>,
>;

type Adapter<N> = NodeAdapter<
    RethFullAdapter<TmpDB, N>,
    <<N as Node<TmpNodeAdapter<N>>>::ComponentsBuilder as NodeComponentsBuilder<
        RethFullAdapter<TmpDB, N>,
    >>::Components,
>;

/// Type alias for a type of `NodeHelper`
pub type NodeHelperType<N, AO> = NodeTestContext<Adapter<N>, AO>;
