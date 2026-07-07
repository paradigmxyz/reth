use reth_chain_state::ExecutedBlock;
use reth_provider::{providers::ProviderNodeTypes, DatabaseProviderFactory, ProviderFactory};
use std::fmt;

/// A hook invoked by the engine persistence task when blocks are saved.
pub trait PersistenceHook<N: ProviderNodeTypes>: Send + Sync {
    /// Invoked before a non-empty `save_blocks` batch is written to storage.
    fn on_save_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[ExecutedBlock<N::Primitives>],
    );
}

impl<F, N> PersistenceHook<N> for F
where
    N: ProviderNodeTypes,
    F: Fn(
            &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
            &[ExecutedBlock<N::Primitives>],
        ) + Send
        + Sync,
{
    fn on_save_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[ExecutedBlock<N::Primitives>],
    ) {
        self(provider, blocks)
    }
}

/// A no-op [`PersistenceHook`] that does nothing.
#[derive(Debug, Default)]
#[non_exhaustive]
pub struct NoopPersistenceHook;

impl<N: ProviderNodeTypes> PersistenceHook<N> for NoopPersistenceHook {
    fn on_save_blocks(
        &self,
        _provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        _blocks: &[ExecutedBlock<N::Primitives>],
    ) {
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
    fn on_save_blocks(
        &self,
        provider: &<ProviderFactory<N> as DatabaseProviderFactory>::ProviderRW,
        blocks: &[ExecutedBlock<N::Primitives>],
    ) {
        for hook in &self.0 {
            hook.on_save_blocks(provider, blocks);
        }
    }
}

/// Boxed [`PersistenceHook`] for the given provider node types.
pub type BoxedPersistenceHook<N> = Box<dyn PersistenceHook<N>>;
