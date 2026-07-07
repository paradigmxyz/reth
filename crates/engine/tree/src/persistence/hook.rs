use alloy_eips::BlockNumHash;
use reth_chain_state::ExecutedBlock;
use reth_errors::ProviderResult;
use reth_provider::{providers::ProviderNodeTypes, DatabaseProviderFactory, ProviderFactory};
use std::fmt;

/// A hook invoked by the engine persistence task when blocks are saved or removed.
///
/// [`Self::save_blocks`] is invoked before the engine persists a non-empty block batch to the
/// database. [`Self::remove_blocks`] is invoked after a non-empty block range has been removed from
/// the database, before the removal is committed.
///
/// Hooks receive the same writable provider used by the persistence operation, so auxiliary writes
/// performed by the hook are committed with the corresponding save or removal.
pub trait PersistenceHook<N: ProviderNodeTypes>: Send + Sync {
    /// Invoked before a non-empty block batch is persisted to the database.
    fn save_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[ExecutedBlock<N::Primitives>],
    ) -> ProviderResult<()>;

    /// Invoked after a non-empty block range is removed from the database, before the removal is
    /// committed.
    fn remove_blocks(
        &self,
        _provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        _blocks: &[BlockNumHash],
    ) -> ProviderResult<()>;
}

/// A no-op [`PersistenceHook`] that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopPersistenceHook;

impl<N: ProviderNodeTypes> PersistenceHook<N> for NoopPersistenceHook {
    fn save_blocks(
        &self,
        _provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        _blocks: &[ExecutedBlock<N::Primitives>],
    ) -> ProviderResult<()> {
        Ok(())
    }

    fn remove_blocks(
        &self,
        _provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        _blocks: &[BlockNumHash],
    ) -> ProviderResult<()> {
        Ok(())
    }
}

/// Multiple [`PersistenceHook`]s that are executed in order.
pub struct PersistenceHooks<N: ProviderNodeTypes>(pub Vec<BoxedPersistenceHook<N>>);

impl<N: ProviderNodeTypes> fmt::Debug for PersistenceHooks<N> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PersistenceHooks").field("len", &self.0.len()).finish()
    }
}

impl<N: ProviderNodeTypes> PersistenceHook<N> for PersistenceHooks<N> {
    fn save_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[ExecutedBlock<N::Primitives>],
    ) -> ProviderResult<()> {
        self.0.iter().try_for_each(|hook| hook.save_blocks(provider, blocks))
    }

    fn remove_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[BlockNumHash],
    ) -> ProviderResult<()> {
        self.0.iter().try_for_each(|hook| hook.remove_blocks(provider, blocks))
    }
}

/// Boxed [`PersistenceHook`] for the given provider node types.
pub type BoxedPersistenceHook<N> = Box<dyn PersistenceHook<N>>;
