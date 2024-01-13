//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes].

use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::Head;
use reth_transaction_pool::TransactionPool;
use std::marker::PhantomData;

/// A type that configures all the customizable components of the node and knows how to build them.
#[async_trait::async_trait]
pub trait NodeComponentsBuilder<Node: FullNodeTypes> {
    /// The transaction pool to use.
    type Pool: TransactionPool;

    // TODO: any other components that are generic?

    /// Builds the components of the node.
    async fn build_components(
        self,
        context: BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>>;
}

/// Captures the necessary context for building the components of the node.
#[derive(Debug, Clone)]
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    head: Head,
    /// The configured provider to interact with the blockchain.
    provider: Node::Provider,

    task_manager: (),

    // TODO maybe combine this with provider
    events: (),

    /// The data dir of the node.
    data_dir: (),
    /// The config of the node
    config: (),
}

impl<Node: FullNodeTypes> BuilderContext<Node> {
    pub fn provider(&self) -> &Node::Provider {
        &self.provider
    }

    /// Returns the current head of the blockchain at launch.
    pub fn head(&self) -> Head {
        self.head
    }

    /// Returns the data dir of the node.
    pub fn data_dir(&self) -> &() {
        &self.data_dir
    }
}

/// A generic, customizable [`NodeComponentsBuilder`].
///
/// This type is stateful and captures the configuration of the node's components.
///
/// ## Component dependencies:
///
/// The components of the node depend on each other:
/// - The payload builder service depends on the transaction pool.
/// - The network depends on the transaction pool.
///
/// We distinguish between different kind of components:
/// - Components that are standalone, such as the transaction pool.
/// - Components that are spawned as a service, such as the payload builder service or the network.
///
/// ## Builder lifecycle:
///
/// First all standalone components are built. Then the service components are spawned.
/// All component builders are captured in the builder state and will be consumed once the node is
/// launched.
#[derive(Debug)]
pub struct ComponentsBuilder<Node, PoolB, PayloadB, NetworkB> {
    pool_builder: PoolB,
    payload_builder: PayloadB,
    network_builder: NetworkB,
    _marker: PhantomData<Node>,
}

// TODO add default impl for ethereum/optimism

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder,
{
}

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder,
    NetworkB: NetworkBuilder<Node, PoolB::Pool>,
    PayloadB: PayloadServiceBuilder<Node, PoolB::Pool>,
{
}

#[async_trait::async_trait]
impl<Node, PoolB, PayloadB, NetworkB> NodeComponentsBuilder<Node>
    for ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder,
    NetworkB: NetworkBuilder<Node, PoolB::Pool>,
    PayloadB: PayloadServiceBuilder<Node, PoolB::Pool>,
{
    type Pool = PoolB::Pool;

    async fn build_components(
        self,
        context: BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>> {
        let Self { pool_builder, payload_builder, network_builder, _marker } = self;

        todo!()
        // let pool = self.pool_builder.build_pool().await?;
        // let network = self.network_builder.build_network().await?;
        // let payload_builder = self.payload_builder.spawn_payload_service().await?;
        // Ok(NodeComponents { transaction_pool: pool, network, payload_builder })
    }
}

/// This captures the
pub struct ComponentsContext<Node: FullNodeTypes> {
    _marker: PhantomData<Node>,
}

/// A type that knows how to build the network implementation.
// TODO: problem: components can depend on each other
#[async_trait::async_trait]
pub trait NetworkBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    async fn build_network(self) -> eyre::Result<NetworkHandle>;
}

/// A type that knows how to build the transaction pool.
#[async_trait::async_trait]
pub trait PoolBuilder: Send {
    /// The transaction pool to build.
    type Pool: TransactionPool;

    async fn build_pool(self) -> eyre::Result<Self::Pool>;
}

/// A type that knows how to spawn the payload service.
#[async_trait::async_trait]
pub trait PayloadServiceBuilder<Node: FullNodeTypes, Pool: TransactionPool>: Send {
    /// Spawns the payload service and returns the handle to it.
    async fn spawn_payload_service(self) -> eyre::Result<PayloadBuilderHandle<Node::Engine>>;
}

// #[async_trait::async_trait]
// impl<F, Engine> PayloadServiceBuilder<Engine> for F
// where
//     Engine: EngineTypes,
//     F: FnOnce() -> eyre::Result<PayloadBuilderHandle<Engine>> + Send,
// {
//     async fn spawn_payload_service(self) -> eyre::Result<PayloadBuilderHandle<Engine>> {
//         self()
//     }
// }

/// All the components of the node.
///
/// This provides access to all the components of the node.
#[derive(Debug)]
pub struct NodeComponents<Node: FullNodeTypes, Pool> {
    /// The transaction pool of the node.
    transaction_pool: Pool,
    /// The network implementation of the node.
    network: NetworkHandle,
    /// The handle to the payload builder service.
    payload_builder: PayloadBuilderHandle<Node::Engine>,
}

impl<Node: FullNodeTypes, Pool> NodeComponents<Node, Pool> {
    /// Returns the handle to the payload builder service.
    pub fn payload_builder(&self) -> PayloadBuilderHandle<Node::Engine> {
        self.payload_builder.clone()
    }
}
