//! Support for configuring the components of a node.
//!
//! Customizable components of the node include:
//!  - The transaction pool.
//!  - The network implementation.
//!  - The payload builder service.
//!
//! Components depend on a fully type configured node: [FullNodeTypes].

use reth_network::NetworkHandle;
use reth_node_api::{node::FullNodeTypes, EngineTypes};
use reth_payload_builder::PayloadBuilderHandle;
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
        partial: PartialComponents<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>>;
}

/// A generic, customizable [`NodeComponentsBuilder`].
///
/// This type is stateful and captures the configuration of the node's components.
///
/// TODO should this also be a state machine?
#[derive(Debug)]
pub struct ComponentsBuilder<Node: FullNodeTypes, PoolB, PayloadB, NetworkB> {
    pool_builder: PoolB,
    payload_builder: PayloadB,
    network_builder: NetworkB,
    _marker: PhantomData<Node>,
}

impl<Node, PoolB, PayloadB, NetworkB> ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder,
    NetworkB: NetworkBuilder,
    PayloadB: PayloadServiceBuilder<Node::Engine>,
{
}

impl<Node, PoolB, PayloadB, NetworkB> NodeComponentsBuilder<Node>
    for ComponentsBuilder<Node, PoolB, PayloadB, NetworkB>
where
    Node: FullNodeTypes,
    PoolB: PoolBuilder,
    NetworkB: NetworkBuilder,
    PayloadB: PayloadServiceBuilder<Node::Engine>,
{
    type Pool = PoolB::Pool;

    async fn build_components(
        self,
        partial: PartialComponents<Node>,
    ) -> eyre::Result<NodeComponents<Node, Self::Pool>> {
        let Self { pool_builder, payload_builder, network_builder, _marker } = self;

        todo!()
        // let pool = self.pool_builder.build_pool().await?;
        // let network = self.network_builder.build_network().await?;
        // let payload_builder = self.payload_builder.spawn_payload_service().await?;
        // Ok(NodeComponents { transaction_pool: pool, network, payload_builder })
    }
}

// TODO we need Builder implementations that can be modified see
// `RethNodeCommandConfig::configure_network`

/// Represents the state of a component during the building process.
///
/// TODO: This could solve the problem of components depending on each other.
#[derive(Debug)]
enum ComponentState<Builder, Value> {
    Pending(Builder),
    Built(Value),
}

/// This captures the
pub struct ComponentsContext<Node: FullNodeTypes> {
    _marker: PhantomData<Node>,
}

/// A type that knows how to build the network implementation.
// TODO: problem: components can depend on each other
#[async_trait::async_trait]
pub trait NetworkBuilder {
    async fn build_network(self) -> eyre::Result<NetworkHandle>;
}

/// A type that knows how to build the transaction pool.
#[async_trait::async_trait]
pub trait PoolBuilder {
    /// The transaction pool to build.
    type Pool: TransactionPool;

    async fn build_pool(self) -> eyre::Result<Self::Pool>;
}

/// A type that knows how to spawn the payload service.
#[async_trait::async_trait]
pub trait PayloadServiceBuilder<Engine: EngineTypes> {
    /// Spawns the payload service and returns the handle to it.
    async fn spawn_payload_service(self) -> eyre::Result<PayloadBuilderHandle<Engine>>;
}

#[async_trait::async_trait]
impl<F, Engine> PayloadServiceBuilder<Engine> for F
where
    Engine: EngineTypes,
    F: FnOnce() -> eyre::Result<PayloadBuilderHandle<Engine>> + Send,
{
    async fn spawn_payload_service(self) -> eyre::Result<PayloadBuilderHandle<Engine>> {
        self()
    }
}

/// A subset of the components of the node.
#[derive(Debug)]
pub struct PartialComponents<Node: FullNodeTypes> {
    provider: Node::Provider,
    // TODO we need executor + events here
}

impl<Node: FullNodeTypes> PartialComponents<Node> {
    /// Returns the provider to interact with the blockchain.
    pub fn provider(&self) -> Node::Provider {
        self.provider.clone()
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
