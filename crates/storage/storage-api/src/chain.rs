use crate::{
    BlockNumReader, BlockReader, DBProvider, HeaderProvider, TransactionsProvider,
    WithdrawalsProvider,
};
use reth_db_api::transaction::{DbTx, DbTxMut};
use reth_primitives::BlockHashOrNumber;
use reth_primitives_traits::NodePrimitives;
use reth_storage_errors::provider::ProviderResult;
use std::fmt::Debug;

/// Trait that implements how complex types (eg. Block) should be read or written to disk.
pub trait ChainStorageReader: Send + Sync + Unpin + Default + Debug {
    /// Primitive types of the node.
    type Primitives: NodePrimitives;

    /// Returns the block with given id from storage.
    ///
    /// Returns `None` if block is not found.
    fn read_block<P>(
        &self,
        provider: &P,
        id: BlockHashOrNumber,
    ) -> ProviderResult<Option<<Self::Primitives as NodePrimitives>::Block>>
    where
        P: DBProvider<Tx: DbTx>
            + TransactionsProvider
            + BlockReader
            + WithdrawalsProvider
            + HeaderProvider
            + BlockNumReader;

    /// Writes block to storage.
    fn write_block<P>(
        &self,
        provider: &P,
        block: &<Self::Primitives as NodePrimitives>::Block,
    ) -> ProviderResult<()>
    where
        P: DBProvider<Tx: DbTxMut>;
}

impl ChainStorageReader for () {
    type Primitives = ();

    fn read_block<P>(
        &self,
        _: &P,
        _: BlockHashOrNumber,
    ) -> ProviderResult<Option<<Self::Primitives as NodePrimitives>::Block>>
    where
        P: DBProvider<Tx: DbTx>
            + TransactionsProvider
            + BlockReader
            + WithdrawalsProvider
            + HeaderProvider
            + BlockNumReader,
    {
        unimplemented!()
    }

    fn write_block<P>(
        &self,
        _: &P,
        _: &<Self::Primitives as NodePrimitives>::Block,
    ) -> ProviderResult<()>
    where
        P: DBProvider<Tx: DbTxMut>,
    {
        unimplemented!()
    }
}
