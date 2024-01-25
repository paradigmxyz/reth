//! Ethereum node types

use crate::{
    components::{ComponentsBuilder, NetworkBuilder, PayloadServiceBuilder, PoolBuilder},
    BuilderContext, EthEngineTypes,
};
use reth_network::NetworkHandle;
use reth_node_api::node::{FullNodeTypes, NodeTypes};
use reth_payload_builder::PayloadBuilderHandle;
use reth_transaction_pool::{blobstore::DiskFileBlobStore, EthTransactionPool, TransactionPool};

/// Type configuration for a regular Ethereum node.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumNode;

impl EthereumNode {
    /// Returns a [ComponentsBuilder] configured for a regular Ethereum node.
    pub fn components<Node: FullNodeTypes>(
    ) -> ComponentsBuilder<Node, EthereumPoolBuilder, EthereumPayloadBuilder, EthereumNetwork> {
        ComponentsBuilder::default()
            .node_types::<Node>()
            .pool(EthereumPoolBuilder::default())
            .payload(EthereumPayloadBuilder::default())
            .network(EthereumNetwork::default())
    }
}

impl NodeTypes for EthereumNode {
    type Primitives = ();
    type Engine = EthEngineTypes;
    type Evm = ();
}

/// A basic ethereum pool.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumPoolBuilder;

impl<Node> PoolBuilder<Node> for EthereumPoolBuilder
where
    Node: FullNodeTypes,
{
    type Pool = EthTransactionPool<Node::Provider, DiskFileBlobStore>;

    fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool> {
        todo!()
    }
}

/// A basic ethereum payload service.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumPayloadBuilder;

impl<Node, Pool> PayloadServiceBuilder<Node, Pool> for EthereumPayloadBuilder
where
    Node: FullNodeTypes,
    Pool: TransactionPool + 'static,
{
    fn spawn_payload_service(
        self,
        ctx: &BuilderContext<Node>,
        pool: Pool,
    ) -> eyre::Result<PayloadBuilderHandle<Node::Engine>> {
        todo!()
    }
}

/// A basic ethereum payload service.
#[derive(Debug, Default, Clone, Copy)]
#[non_exhaustive]
pub struct EthereumNetwork;

impl<Node, Pool> NetworkBuilder<Node, Pool> for EthereumNetwork
where
    Node: FullNodeTypes,
    Pool: TransactionPool + 'static,
{
    fn build_network(self, ctx: &BuilderContext<Node>, pool: Pool) -> eyre::Result<NetworkHandle> {
        todo!()
    }
}
