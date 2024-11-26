use crate::{providers::NodeTypesForProvider, DatabaseProvider};
use reth_db::transaction::{DbTx, DbTxMut};
use reth_node_types::FullNodePrimitives;
use reth_primitives::EthPrimitives;
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

impl ChainStorage<EthPrimitives> for EthStorage {
    fn reader<TX, Types>(
        &self,
    ) -> impl ChainStorageReader<DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = EthPrimitives>,
    {
        self
    }

    fn writer<TX, Types>(
        &self,
    ) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypesForProvider<Primitives = EthPrimitives>,
    {
        self
    }
}
