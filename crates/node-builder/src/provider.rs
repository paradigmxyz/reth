//! Helper provider traits to encapsulate all provider traits for simplicity.

use reth_db::database::Database;
use reth_provider::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    DatabaseProviderFactory, EvmEnvProvider, StateProviderFactory,
};

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider<DB: Database>:
    DatabaseProviderFactory<DB>
    + BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider
    + ChangeSetReader
    + CanonStateSubscriptions
    + Clone
    + Unpin
    + 'static
{
}

impl<T, DB: Database> FullProvider<DB> for T where
    T: DatabaseProviderFactory<DB>
        + BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + CanonStateSubscriptions
        + Clone
        + Unpin
        + 'static
{
}
