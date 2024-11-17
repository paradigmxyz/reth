use crate::{providers::NodeTypes, DatabaseProvider};
use reth_db::transaction::{DbTx, DbTxMut};
use reth_node_types::FullNodePrimitives;
use reth_primitives::EthPrimitives;
use reth_storage_api::{ChainStorageWriter, EthStorage};

/// Trait that implements how chain primitives are written to storage.
pub trait ChainStorage<Primitives: FullNodePrimitives>: Send + Sync {
    /// The chain writer implementation.
    type Writer<TX: DbTxMut + DbTx + 'static, Types: NodeTypes<Primitives = Primitives>>: ChainStorageWriter<
        DatabaseProvider<TX, Types>,
        Primitives,
    >;

    /// Provides access to the chain writer.
    fn writer<TX: DbTxMut + DbTx + 'static, Types: NodeTypes<Primitives = Primitives>>(
        &self,
    ) -> &Self::Writer<TX, Types>;
}

impl ChainStorage<EthPrimitives> for EthStorage {
    type Writer<TX: DbTxMut + DbTx + 'static, Types: NodeTypes<Primitives = EthPrimitives>> = Self;

    fn writer<TX: DbTxMut + DbTx + 'static, Types: NodeTypes<Primitives = EthPrimitives>>(
        &self,
    ) -> &Self::Writer<TX, Types> {
        self
    }
}
