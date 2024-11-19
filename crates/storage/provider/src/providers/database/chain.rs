use crate::{providers::NodeTypes, DatabaseProvider};
use reth_db::transaction::{DbTx, DbTxMut};
use reth_node_types::FullNodePrimitives;
use reth_primitives::EthPrimitives;
use reth_storage_api::{ChainStorageWriter, EthStorage};

/// Trait that provides access to implementations of [`ChainStorage`]
pub trait ChainStorage<Primitives: FullNodePrimitives>: Send + Sync {
    /// Provides access to the chain writer.
    fn writer<TX, Types>(&self) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, Primitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypes<Primitives = Primitives>;
}

impl ChainStorage<EthPrimitives> for EthStorage {
    fn writer<TX, Types>(
        &self,
    ) -> impl ChainStorageWriter<DatabaseProvider<TX, Types>, EthPrimitives>
    where
        TX: DbTxMut + DbTx + 'static,
        Types: NodeTypes<Primitives = EthPrimitives>,
    {
        self
    }
}
