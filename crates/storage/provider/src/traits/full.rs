//! Helper provider traits to encapsulate all provider traits for simplicity.

use crate::{
    AccountReader, BlockReaderIdExt, CanonStateSubscriptions, ChainSpecProvider, ChangeSetReader,
    DatabaseProviderFactory, EvmEnvProvider, StateProviderFactory,
};
use reth_db::database::Database;

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
