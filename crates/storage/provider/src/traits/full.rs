//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    AccountReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    EvmEnvProvider, HeaderProvider, StageCheckpointReader, StateProviderFactory,
    StaticFileProviderFactory, TransactionsProvider,
};
use reth_chain_state::{CanonStateSubscriptions, ForkChoiceSubscriptions};
use reth_chainspec::EthereumHardforks;
use reth_node_types::NodeTypesWithDB;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<N: NodeTypesWithDB>:
    DatabaseProviderFactory<DB = N::DB>
    + StaticFileProviderFactory
    + BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider<ChainSpec = N::ChainSpec>
    + ChangeSetReader
    + CanonStateSubscriptions
    + ForkChoiceSubscriptions
    + StageCheckpointReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T, N: NodeTypesWithDB> FullProvider<N> for T where
    T: DatabaseProviderFactory<DB = N::DB>
        + StaticFileProviderFactory
        + BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider<ChainSpec = N::ChainSpec>
        + ChangeSetReader
        + CanonStateSubscriptions
        + ForkChoiceSubscriptions
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
    + EvmEnvProvider
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
        + EvmEnvProvider
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
