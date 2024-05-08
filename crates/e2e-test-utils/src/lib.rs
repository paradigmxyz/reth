use crate::node::NodeTestContext;
use reth::{
    args::{DiscoveryArgs, NetworkArgs, RpcServerArgs},
    builder::{NodeBuilder, NodeConfig, NodeHandle},
    tasks::TaskExecutor,
};
use reth_db::{test_utils::TempDatabase, DatabaseEnv};
use reth_node_builder::{
    components::NodeComponentsBuilder, FullNodeTypesAdapter, Node, NodeAdapter, RethFullAdapter,
};
use reth_primitives::ChainSpec;
use reth_provider::providers::BlockchainProvider;
use std::{marker::PhantomData, sync::Arc};
use tracing::{span, Level};

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

pub mod runner;

/// Builder for creating a network of nodes for testing.
/// The network is created with a chain spec and a number of peers.
/// The nodes are interconnected in a chain.
pub struct TestNetworkBuilder<N: Default + reth_node_builder::Node<TmpNodeAdapter<N>>>
where
    N: Node<TmpNodeAdapter<N>>,
{
    /// `Network` is a vector that holds the context of each node in the network.
    /// Each `NodeTestCtx` in the vector represents a node in the network.
    network: Vec<NodeTestCtx<N>>,

    /// `Node_generator` is an instance of `TestNodeGenerator` that is used to generate new nodes.
    /// It is initialized with a chain specification and an executor, and can be used to generate
    /// nodes for the network.
    node_generator: TestNodeGenerator<N>,
}

impl<N> TestNetworkBuilder<N>
where
    N: Default + Node<TmpNodeAdapter<N>>,
{
    /// Create a new network builder with a number of peers, a chain spec and an executor.
    pub fn new(peers: usize, chain_spec: Arc<ChainSpec>, exec: TaskExecutor) -> Self {
        let node_generator = TestNodeGenerator::new(chain_spec, exec);
        Self { network: Vec::with_capacity(peers), node_generator }
    }

    /// Builds the network of nodes in a chain connected via p2p.
    pub async fn build(mut self) -> eyre::Result<Vec<NodeTestCtx<N>>> {
        let len = self.network.capacity();
        for node_index in 0..len {
            let span = span!(Level::INFO, "node", node_index);
            let _enter = span.enter();
            let mut node = self.node_generator.gen().await?;
            self.connect_nodes(&mut node, node_index).await;

            self.network.push(node);
        }
        Ok(self.network)
    }

    /// Connects the nodes in the network in a chain.
    async fn connect_nodes(&mut self, node: &mut NodeTestCtx<N>, node_index: usize) {
        // Connect with the previous node if it exists
        if let Some(previous_node) = self.network.last_mut() {
            previous_node.network.connect(node).await;
        }

        // Connect with the first node if this is the last node and there are more than two nodes
        if node_index + 1 == self.network.capacity() && self.network.capacity() > 2 {
            if let Some(first_node) = self.network.first_mut() {
                node.network.connect(first_node).await;
            }
        }
    }
}

/// Generator for creating a test node with a chain spec and an executor.
pub struct TestNodeGenerator<N>
where
    N: Default + Node<TmpNodeAdapter<N>>,
{
    chain_spec: Arc<ChainSpec>,
    exec: TaskExecutor,
    dev: bool,
    _marker: PhantomData<N>,
}

impl<N> TestNodeGenerator<N>
where
    N: Default + Node<TmpNodeAdapter<N>>,
{
    pub fn new(chain_spec: Arc<ChainSpec>, exec: TaskExecutor) -> Self {
        Self { chain_spec, exec, _marker: PhantomData, dev: false }
    }

    /// Sets the node to run in dev mode.
    pub fn dev(mut self) -> Self {
        self.dev = true;
        self
    }

    /// Generates a new test node with p2p discovery disabled.
    pub async fn gen(&self) -> eyre::Result<NodeTestCtx<N>> {
        let network_config = NetworkArgs {
            discovery: DiscoveryArgs { disable_discovery: true, ..DiscoveryArgs::default() },
            ..NetworkArgs::default()
        };
        let mut node_config = NodeConfig::test()
            .with_chain(self.chain_spec.clone())
            .with_network(network_config.clone())
            .with_unused_ports()
            .with_rpc(RpcServerArgs::default().with_unused_ports().with_http());

        if self.dev {
            node_config = node_config.dev();
        }

        let NodeHandle { node, node_exit_future: _ } = NodeBuilder::new(node_config.clone())
            .testing_node(self.exec.clone())
            .node(Default::default())
            .launch()
            .await?;

        let node = NodeTestContext::new(node).await?;

        Ok(node)
    }
}

// Type aliases
type TmpDB = Arc<TempDatabase<DatabaseEnv>>;
type TmpNodeAdapter<N> = FullNodeTypesAdapter<N, TmpDB, BlockchainProvider<TmpDB>>;
type Adapter<N> = NodeAdapter<
    RethFullAdapter<TmpDB, N>,
    <<N as Node<TmpNodeAdapter<N>>>::ComponentsBuilder as NodeComponentsBuilder<
        RethFullAdapter<TmpDB, N>,
    >>::Components,
>;
pub type NodeTestCtx<N> = NodeTestContext<Adapter<N>>;
