use crate::{providers::NodeTypesForProvider, DatabaseProvider};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_node_types::FullNodePrimitives;

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

impl<N> ChainStorage<N>
    for EthStorage<reth_ethereum_primitives::TransactionSigned, reth_primitives_traits::Header>
where
    N: FullNodePrimitives<
        Block = reth_ethereum_primitives::Block,
        BlockHeader = reth_primitives_traits::Header,
        BlockBody = reth_ethereum_primitives::BlockBody,
        SignedTx = reth_ethereum_primitives::TransactionSigned,
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
