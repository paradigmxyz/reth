//! Pool component for the node builder.
use crate::components::BuilderContext;
use reth_node_api::node::FullNodeTypes;
use reth_transaction_pool::{blobstore::InMemoryBlobStore, TransactionPool};
use std::marker::PhantomData;

/// A type that knows how to build the transaction pool.
pub trait PoolBuilder<Node: FullNodeTypes>: Send {
    /// The transaction pool to build.
    type Pool: TransactionPool;

    /// Creates the transaction pool.
    fn build_pool(self, ctx: &BuilderContext<Node>) -> eyre::Result<Self::Pool>;
}

/// A basic [PoolBuilder] for an ethereum node.
#[derive(Debug)]
pub struct EthereumPoolBuilder<B = MemoryBlobstore> {
    _marker: PhantomData<B>,
}

impl<B> EthereumPoolBuilder<B> {
    /// Configures the transaction pool to use an in-memory blobstore.
    pub fn in_memory_blobstore(self) -> EthereumPoolBuilder<InMemoryBlobStore> {
        EthereumPoolBuilder { _marker: Default::default() }
    }

    /// Configures the transaction pool to use an in-memory blobstore.
    pub fn on_disk_blobstore(self) -> EthereumPoolBuilder<DiskBlobstore> {
        EthereumPoolBuilder { _marker: Default::default() }
    }
}

impl Default for EthereumPoolBuilder {
    fn default() -> Self {
        EthereumPoolBuilder { _marker: Default::default() }
    }
}

// TODO impl PoolBuilder for EthereumPoolBuilder

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct MemoryBlobstore;

#[derive(Debug, Default)]
#[non_exhaustive]
pub struct DiskBlobstore;
