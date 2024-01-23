//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes].

pub use network::*;
pub use payload::*;
pub use pool::*;
use reth_network::NetworkHandle;
use reth_node_api::node::FullNodeTypes;
use reth_payload_builder::PayloadBuilderHandle;
use reth_primitives::Head;
use reth_tasks::TaskExecutor;
use reth_transaction_pool::TransactionPool;
use std::marker::PhantomData;

mod network;
mod payload;
mod pool;

/// A type that configures all the customizable components of the node and knows how to build them.
///
/// Implementors of this trait are responsible for building all the components of the node: See
/// [NodeComponents].
///
/// The [ComponentsBuilder] is a generic implementation of this trait that can be used to customize
/// certain components of the node using the builder pattern and defaults, e.g. Ethereum and
/// Optimism.
pub trait NodeComponentsBuilder<Node: FullNodeTypes> {
    /// The transaction pool to use.
    type Pool: TransactionPool;

    /// Builds the components of the node.
    fn build_components(
        self,
        context: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>>;
}

impl<Node, F, Pool> NodeComponentsBuilder<Node> for F
where
    Node: FullNodeTypes,
    F: FnOnce(&BuilderContext<Node>) -> eyre::Result<NodeComponents<Node, Pool>>,
    Pool: TransactionPool,
{
    type Pool = Pool;

    fn build_components(
        self,
        ctx: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Pool>> {
        self(ctx)
    }
}

/// Captures the necessary context for building the components of the node.
#[derive(Debug)]
pub struct BuilderContext<Node: FullNodeTypes> {
    /// The current head of the blockchain at launch.
    head: Head,
    /// The configured provider to interact with the blockchain.
    provider: Node::Provider,

    executor: TaskExecutor,

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

    // TODO read only helper methods to access the config traits (cli args)
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

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB> {
    /// Configures the node types.
    pub fn node_types<Types>(self) -> ComponentsBuilder<Types, PoolB, PayloadB, NetworkB>
    where
        Types: FullNodeTypes,
    {
        let Self { pool_builder, payload_builder, network_builder, _marker } = self;
        ComponentsBuilder {
            pool_builder,
            payload_builder,
            network_builder,
            _marker: Default::default(),
        }
    }
}

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
{
    /// Configures the pool builder.
    pub fn pool<PB>(self, pool_builder: PB) -> ComponentsBuilder<Node, PB, PayloadB, NetworkB>
    where
        PB: PoolBuilder<Node>,
    {
        let Self { payload_builder, network_builder, _marker, .. } = self;
        ComponentsBuilder { pool_builder, payload_builder, network_builder, _marker }
    }
}

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder<Node>,
{
    /// Configures the network builder.
    pub fn network<NB>(self, network_builder: NB) -> ComponentsBuilder<Node, PoolB, PayloadB, NB>
    where
        NB: NetworkBuilder<Node, PoolB::Pool>,
    {
        let Self { payload_builder, pool_builder, _marker, .. } = self;
        ComponentsBuilder { pool_builder, payload_builder, network_builder, _marker }
    }

    /// Configures the payload builder.
    pub fn payload<PB>(self, payload_builder: PB) -> ComponentsBuilder<Node, PoolB, PB, NetworkB>
    where
        PB: PayloadServiceBuilder<Node, PoolB::Pool>,
    {
        let Self { pool_builder, network_builder, _marker, .. } = self;
        ComponentsBuilder { pool_builder, payload_builder, network_builder, _marker }
    }
}

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder<Node>,
    NetworkB: NetworkBuilder<Node, PoolB::Pool>,
    PayloadB: PayloadServiceBuilder<Node, PoolB::Pool>,
{
}

impl<Node, PoolB, PayloadB, NetworkB> NodeComponentsBuilder<Node>
    for ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder<Node>,
    NetworkB: NetworkBuilder<Node, PoolB::Pool>,
    PayloadB: PayloadServiceBuilder<Node, PoolB::Pool>,
{
    type Pool = PoolB::Pool;

    fn build_components(
        self,
        context: &BuilderContext<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>> {
        let Self { pool_builder, payload_builder, network_builder, _marker } = self;

        let pool = pool_builder.build_pool(context)?;
        let network = network_builder.build_network(context, pool.clone())?;
        let payload_builder = payload_builder.spawn_payload_service(context, pool.clone())?;

        Ok(NodeComponents { transaction_pool: pool, network, payload_builder })
    }
}

impl Default for ComponentsBuilder<(), (), (), ()> {
    fn default() -> Self {
        Self {
            pool_builder: (),
            payload_builder: (),
            network_builder: (),
            _marker: Default::default(),
        }
    }
}

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
