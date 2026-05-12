//! Dispatcher trait for incremental state-root computation.
//!
//! This trait lets the merkle stage call into a parallel implementation without
//! threading the underlying `Factory` generic through its type signatures. The
//! concrete [`ParallelStateRootDispatcher`] wraps the factory + runtime so the
//! caller only needs to hold an `Arc<dyn IncrementalStateRootDispatcher>`.

use crate::root::{ParallelStateRoot, ParallelStateRootError};
use alloy_primitives::B256;
use reth_provider::{
    providers::OverlayStateProviderFactory, BlockNumReader, ChangeSetReader,
    DatabaseProviderFactory, DatabaseProviderROFactory, PruneCheckpointReader,
    StageCheckpointReader, StorageChangeSetReader, StorageSettingsCache,
};
use reth_tasks::Runtime;
use reth_trie::{
    hashed_cursor::HashedCursorFactory,
    prefix_set::TriePrefixSets,
    trie_cursor::TrieCursorFactory,
    updates::{TrieUpdates, TrieUpdatesSorted},
};
use std::{fmt::Debug, sync::Arc};

/// Type-erased entry point for parallel incremental state-root computation.
///
/// Consumers (e.g. the merkle stage) hold this as `Arc<dyn IncrementalStateRootDispatcher>`
/// so the underlying factory generic does not leak into their type signatures.
pub trait IncrementalStateRootDispatcher: Debug + Send + Sync {
    /// Compute the incremental state root + trie updates for the given prefix sets.
    fn incremental_root_with_updates(
        &self,
        prefix_sets: TriePrefixSets,
        trie_updates_overlay: Option<Arc<TrieUpdatesSorted>>,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError>;
}

/// Default dispatcher backed by [`ParallelStateRoot`].
#[derive(Debug, Clone)]
pub struct ParallelStateRootDispatcher<F> {
    overlay_factory: OverlayStateProviderFactory<F>,
    runtime: Runtime,
}

impl<F> ParallelStateRootDispatcher<F> {
    /// Create a new dispatcher from an overlay factory and a runtime.
    pub const fn new(overlay_factory: OverlayStateProviderFactory<F>, runtime: Runtime) -> Self {
        Self { overlay_factory, runtime }
    }
}

impl<F> IncrementalStateRootDispatcher for ParallelStateRootDispatcher<F>
where
    F: DatabaseProviderFactory + Debug + Clone + Send + Sync + 'static,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
        + BlockNumReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache,
    OverlayStateProviderFactory<F>: DatabaseProviderROFactory<Provider: TrieCursorFactory + HashedCursorFactory>
        + Clone
        + Send
        + Sync
        + 'static,
{
    fn incremental_root_with_updates(
        &self,
        prefix_sets: TriePrefixSets,
        trie_updates_overlay: Option<Arc<TrieUpdatesSorted>>,
    ) -> Result<(B256, TrieUpdates), ParallelStateRootError> {
        let overlay_factory = match trie_updates_overlay {
            Some(trie) if !trie.is_empty() => {
                self.overlay_factory.clone().with_trie_updates_overlay(Some(trie))
            }
            _ => self.overlay_factory.clone(),
        };

        ParallelStateRoot::new(overlay_factory, prefix_sets, self.runtime.clone())
            .incremental_root_with_updates()
    }
}
