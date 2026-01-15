//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    AccountReader, BlockReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader,
    DatabaseProviderFactory, HashedPostStateProvider, PruneCheckpointReader,
    RocksDBProviderFactory, StageCheckpointReader, StateProviderFactory, StateReader,
    StaticFileProviderFactory, TrieReader,
};
use reth_chain_state::{
    CanonStateSubscriptions, ForkChoiceSubscriptions, PersistedBlockSubscriptions,
};
use reth_node_types::{BlockTy, HeaderTy, NodeTypesWithDB, ReceiptTy, TxTy};
use reth_storage_api::{NodePrimitivesProvider, StorageChangeSetReader};
use std::fmt::Debug;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<N: NodeTypesWithDB>:
    DatabaseProviderFactory<
        DB = N::DB,
        Provider: BlockReader
                      + TrieReader
                      + StageCheckpointReader
                      + PruneCheckpointReader
                      + ChangeSetReader
                      + StorageChangeSetReader,
    > + NodePrimitivesProvider<Primitives = N::Primitives>
    + StaticFileProviderFactory<Primitives = N::Primitives>
    + RocksDBProviderFactory
    + BlockReaderIdExt<
        Transaction = TxTy<N>,
        Block = BlockTy<N>,
        Receipt = ReceiptTy<N>,
        Header = HeaderTy<N>,
    > + AccountReader
    + StateProviderFactory
    + StateReader
    + HashedPostStateProvider
    + ChainSpecProvider<ChainSpec = N::ChainSpec>
    + ChangeSetReader
    + StorageChangeSetReader
    + CanonStateSubscriptions
    + ForkChoiceSubscriptions<Header = HeaderTy<N>>
    + PersistedBlockSubscriptions
    + StageCheckpointReader
    + Clone
    + Debug
    + Unpin
    + 'static
{
}

impl<T, N: NodeTypesWithDB> FullProvider<N> for T where
    T: DatabaseProviderFactory<
            DB = N::DB,
            Provider: BlockReader
                          + TrieReader
                          + StageCheckpointReader
                          + PruneCheckpointReader
                          + ChangeSetReader
                          + StorageChangeSetReader,
        > + NodePrimitivesProvider<Primitives = N::Primitives>
        + StaticFileProviderFactory<Primitives = N::Primitives>
        + RocksDBProviderFactory
        + BlockReaderIdExt<
            Transaction = TxTy<N>,
            Block = BlockTy<N>,
            Receipt = ReceiptTy<N>,
            Header = HeaderTy<N>,
        > + AccountReader
        + StateProviderFactory
        + StateReader
        + HashedPostStateProvider
        + ChainSpecProvider<ChainSpec = N::ChainSpec>
        + ChangeSetReader
        + StorageChangeSetReader
        + CanonStateSubscriptions
        + ForkChoiceSubscriptions<Header = HeaderTy<N>>
        + PersistedBlockSubscriptions
        + StageCheckpointReader
        + Clone
        + Debug
        + Unpin
        + 'static
{
}
