//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    AccountReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, DatabaseProviderFactory,
    EvmEnvProvider, HeaderProvider, StageCheckpointReader, StateProviderFactory,
    StaticFileProviderFactory, TransactionsProvider,
};
use reth_chain_state::{CanonStateSubscriptions, ForkChoiceSubscriptions};
use reth_chainspec::{ChainSpec, EthChainSpec};
use reth_db_api::database::Database;

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<DB: Database, ChainSpec: EthChainSpec>:
    DatabaseProviderFactory<DB>
    + StaticFileProviderFactory
    + BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider<ChainSpec = ChainSpec>
    + ChangeSetReader
    + CanonStateSubscriptions
    + ForkChoiceSubscriptions
    + StageCheckpointReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T, DB: Database, ChainSpec: EthChainSpec> FullProvider<DB, ChainSpec> for T where
    T: DatabaseProviderFactory<DB>
        + StaticFileProviderFactory
        + BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider<ChainSpec = ChainSpec>
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
    + ChainSpecProvider<ChainSpec = ChainSpec>
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
        + ChainSpecProvider<ChainSpec = ChainSpec>
        + BlockReaderIdExt
        + HeaderProvider
        + TransactionsProvider
        + StageCheckpointReader
        + Clone
        + Unpin
        + 'static
{
}
