//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    AccountReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    HeaderProvider, StageCheckpointReader, StateProviderFactory, StaticFileProviderFactory,
    TransactionsProvider,
};
use reth_chain_state::{CanonStateSubscriptions, ForkChoiceSubscriptions};
use reth_chainspec::EthereumHardforks;
use reth_node_types::{BlockTy, HeaderTy, NodeTypesWithDB, ReceiptTy, TxTy};
use reth_storage_api::NodePrimitivesProvider;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<N: NodeTypesWithDB>:
    DatabaseProviderFactory<DB = N::DB>
    + NodePrimitivesProvider<Primitives = N::Primitives>
    + StaticFileProviderFactory
    + BlockReaderIdExt<
        Transaction = TxTy<N>,
        Block = BlockTy<N>,
        Receipt = ReceiptTy<N>,
        Header = HeaderTy<N>,
    > + AccountReader
    + StateProviderFactory
    + ChainSpecProvider<ChainSpec = N::ChainSpec>
    + ChangeSetReader
    + CanonStateSubscriptions
    + ForkChoiceSubscriptions<Header = HeaderTy<N>>
    + StageCheckpointReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T, N: NodeTypesWithDB> FullProvider<N> for T where
    T: DatabaseProviderFactory<DB = N::DB>
        + NodePrimitivesProvider<Primitives = N::Primitives>
        + StaticFileProviderFactory
        + BlockReaderIdExt<
            Transaction = TxTy<N>,
            Block = BlockTy<N>,
            Receipt = ReceiptTy<N>,
            Header = HeaderTy<N>,
        > + AccountReader
        + StateProviderFactory
        + ChainSpecProvider<ChainSpec = N::ChainSpec>
        + ChangeSetReader
        + CanonStateSubscriptions
        + ForkChoiceSubscriptions<Header = HeaderTy<N>>
        + StageCheckpointReader
        + Clone
        + Unpin
        + 'static
{
}

/// Helper trait to unify all provider traits required to support `eth` RPC server behaviour, for
/// simplicity.
pub trait FullRpcProvider:
    StateProviderFactory
    + ChainSpecProvider<ChainSpec: EthereumHardforks>
    + BlockReaderIdExt
    + HeaderProvider
    + TransactionsProvider
    + StageCheckpointReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T> FullRpcProvider for T where
    T: StateProviderFactory
        + ChainSpecProvider<ChainSpec: EthereumHardforks>
        + BlockReaderIdExt
        + HeaderProvider
        + TransactionsProvider
        + StageCheckpointReader
        + Clone
        + Unpin
        + 'static
{
}
