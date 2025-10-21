use crate::{providers::NodeTypesForProvider, DatabaseProvider};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_node_types::NodePrimitives;

use reth_primitives_traits::{FullBlockHeader, FullSignedTx};
use reth_storage_api::{ChainStorageReader, ChainStorageWriter, EmptyBodyStorage, EthStorage};

/// Trait that provides access to implementations of [`ChainStorage`]
pub trait ChainStorage<N: NodePrimitives>: Send + Sync {
    /// Provides access to the chain reader.
    fn reader<TX, Types>(&self) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, N>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = N>;

    /// Provides access to the chain writer.
    fn writer<TX, Types>(&self) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, N>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = N>;
}

impl<N, T, H> ChainStorage<N> for EthStorage
where
    T: FullSignedTx,
    H: FullBlockHeader,
    N: NodePrimitives<
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

impl<N, T, H> ChainStorage<N> for EmptyBodyStorage
where
    T: FullSignedTx,
    H: FullBlockHeader,
    N: NodePrimitives<BlockHeader = H, BlockBody = alloy_consensus::BlockBody<T, H>, SignedTx = T>,
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
