//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    BalProvider, BlockReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader,
    DatabaseProviderFactory, PruneCheckpointReader, RocksDBProviderFactory, StageCheckpointReader,
    StateProviderFactory, StateRangeProviderFactory, StateReader, StaticFileProviderFactory,
};
use reth_chain_state::{
    CanonStateSubscriptions, ForkChoiceSubscriptions, PersistedBlockSubscriptions,
};
use reth_node_types::{BlockTy, HeaderTy, NodeTypesWithDB, ReceiptTy, TxTy};
use reth_storage_api::{
    NodePrimitivesProvider, StorageChangeSetReader, StorageSettingsCache,
    TryIntoHistoricalStateProvider,
};
use std::fmt::Debug;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<N: NodeTypesWithDB>:
    DatabaseProviderFactory<
        DB = N::DB,
        Provider: BlockReader
                      + StageCheckpointReader
                      + PruneCheckpointReader
                      + ChangeSetReader
                      + StorageChangeSetReader
                      + StorageSettingsCache
                      + TryIntoHistoricalStateProvider
                      + 'static,
    > + NodePrimitivesProvider<Primitives = N::Primitives>
    + StaticFileProviderFactory<Primitives = N::Primitives>
    + RocksDBProviderFactory
    + BlockReaderIdExt<
        Transaction = TxTy<N>,
        Block = BlockTy<N>,
        Receipt = ReceiptTy<N>,
        Header = HeaderTy<N>,
    > + BalProvider
    + StateProviderFactory
    + StateRangeProviderFactory
    + StateReader
    + ChainSpecProvider<ChainSpec = N::ChainSpec>
    + ChangeSetReader
    + StorageChangeSetReader
    + CanonStateSubscriptions
    + ForkChoiceSubscriptions<Header = HeaderTy<N>>
    + PersistedBlockSubscriptions
    + StageCheckpointReader
    + PruneCheckpointReader
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
                          + StageCheckpointReader
                          + PruneCheckpointReader
                          + ChangeSetReader
                          + StorageChangeSetReader
                          + StorageSettingsCache
                          + TryIntoHistoricalStateProvider
                          + 'static,
        > + NodePrimitivesProvider<Primitives = N::Primitives>
        + StaticFileProviderFactory<Primitives = N::Primitives>
        + RocksDBProviderFactory
        + BlockReaderIdExt<
            Transaction = TxTy<N>,
            Block = BlockTy<N>,
            Receipt = ReceiptTy<N>,
            Header = HeaderTy<N>,
        > + BalProvider
        + StateProviderFactory
        + StateRangeProviderFactory
        + StateReader
        + ChainSpecProvider<ChainSpec = N::ChainSpec>
        + ChangeSetReader
        + StorageChangeSetReader
        + CanonStateSubscriptions
        + ForkChoiceSubscriptions<Header = HeaderTy<N>>
        + PersistedBlockSubscriptions
        + StageCheckpointReader
        + PruneCheckpointReader
        + Clone
        + Debug
        + Unpin
        + 'static
{
}
