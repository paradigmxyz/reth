use reth_provider::{
    AccountReader, BlockReaderIdExt, ChainSpecProvider, ChangeSetReader, EvmEnvProvider,
    StateProviderFactory,
};

/// Helper trait to unify all provider traits for simplicity.
pub trait FullProvider:
    BlockReaderIdExt
    + AccountReader
    + StateProviderFactory
    + EvmEnvProvider
    + ChainSpecProvider
    + ChangeSetReader
    + Clone
    + Unpin
    + 'static
{
}

impl<T> FullProvider for T where
    T: BlockReaderIdExt
        + AccountReader
        + StateProviderFactory
        + EvmEnvProvider
        + ChainSpecProvider
        + ChangeSetReader
        + Clone
        + Unpin
        + 'static
{
}
