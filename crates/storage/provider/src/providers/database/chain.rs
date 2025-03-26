use crate::{providers::NodeTypesForProvider, DatabaseProvider};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_node_types::FullNodePrimitives;

use reth_primitives_traits::{FullBlockHeader, FullSignedTx};
use reth_storage_api::{ChainStorageReader, ChainStorageWriter, EthStorage};

/// Trait that provides access to implementations of [`ChainStorage`]
pub trait ChainStorage<Primitives: FullNodePrimitives>: Send + Sync {
    /// Provides access to the chain reader.
    fn reader<TX, Types>(&self) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, Primitives>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = Primitives>;

    /// Provides access to the chain writer.
    fn writer<TX, Types>(&self) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, Primitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = Primitives>;
}

impl<N, T, H> ChainStorage<N> for EthStorage<T, H>
where
    T: FullSignedTx,
    H: FullBlockHeader,
    N: FullNodePrimitives<
        Block = alloy_consensus::Block<T, H>,
        BlockHeader = H,
        BlockBody = alloy_consensus::BlockBody<T, H>,
        SignedTx = T,
    >,
{
    fn reader<TX, Types>(&self) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, N>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = N>,
    {
        self
    }

    fn writer<TX, Types>(&self) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, N>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = N>,
    {
        self
    }
}
