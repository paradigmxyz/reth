use crate::{database_state_frontiers, ExecutionOverlay, OverlayBuilder, StateTrieOverlay};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, B256};
use metrics::{Counter, Histogram};
use reth_db_api::{tables, transaction::DbTx, DatabaseError};
use reth_errors::ProviderResult;
use reth_ethereum_primitives::EthPrimitives;
use reth_metrics::Metrics;
use reth_primitives_traits::{
    dashmap::{self, DashMap},
    NodePrimitives,
};
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
    DatabaseProviderROFactory, DbTxProvider, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursor, TrieCursor, TrieCursorFactory, TrieStorageCursor},
    HashedPostStateSorted,
};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseHashedCursorFactory, DatabaseStorageTrieCursor,
    LegacyKeyAdapter, PackedAccountsTrie, PackedKeyAdapter, PackedStoragesTrie,
};
use std::{fmt, marker::PhantomData, sync::Arc, time::Instant};
use tracing::instrument;

/// Metrics for overlay state provider factory operations.
#[derive(Clone, Metrics)]
#[metrics(scope = "storage.providers.overlay")]
pub(crate) struct OverlayStateProviderFactoryMetrics {
    /// Duration of creating the database provider transaction.
    create_provider_duration: Histogram,
    /// Overall duration of the [`OverlayStateProviderFactory::database_provider_ro`] call.
    database_provider_ro_duration: Histogram,
    /// Number of cache misses when fetching state trie overlays.
    state_trie_overlay_cache_misses: Counter,
    /// Number of cache misses when fetching execution overlays.
    execution_overlay_cache_misses: Counter,
}

/// Factory for creating overlay state providers with optional reverts and overlays.
///
/// This factory allows building an `OverlayStateProvider` whose DB state has been reverted to a
/// particular block, and/or with additional overlay information added on top.
#[derive(Debug, Clone)]
pub struct OverlayStateProviderFactory<
    F,
    N: NodePrimitives = EthPrimitives,
    OverlayKind = StateTrieOverlayOnly,
> {
    /// The underlying database provider factory
    factory: F,
    /// Overlay builder containing the configuration and overlay calculation logic.
    overlay_builder: OverlayBuilder<N>,
    /// A cache which maps durable frontier pairs to [`StateTrieOverlay`]s.
    ///
    /// Under partial persistence the overlay depends on both durable frontiers, so both hashes are
    /// part of the cache key.
    state_trie_overlay_cache: Arc<DashMap<(BlockHash, BlockHash), StateTrieOverlay>>,
    /// A cache which maps durable frontier pairs to [`ExecutionOverlay`]s.
    execution_overlay_cache: Arc<DashMap<(BlockHash, BlockHash), ExecutionOverlay>>,
    /// Metrics for provider factory operations.
    metrics: OverlayStateProviderFactoryMetrics,
    _kind: PhantomData<OverlayKind>,
}

impl<F, N: NodePrimitives, OverlayKind> OverlayStateProviderFactory<F, N, OverlayKind> {
    fn with_kind(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self {
            factory,
            overlay_builder,
            state_trie_overlay_cache: Default::default(),
            execution_overlay_cache: Default::default(),
            metrics: Default::default(),
            _kind: PhantomData,
        }
    }

    /// Skips managed overlay construction when this factory is used by a task that reused a sparse
    /// trie covering both durable frontiers through the parent.
    pub fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.overlay_builder =
            self.overlay_builder.with_skip_overlay_for_reused_sparse_trie(anchor_hash);
        self.state_trie_overlay_cache = Default::default();
        self.execution_overlay_cache = Default::default();
        self
    }
}

impl<F, N: NodePrimitives> OverlayStateProviderFactory<F, N, StateTrieOverlayOnly> {
    /// Create a new overlay state provider factory
    pub fn new(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self::with_kind(factory, overlay_builder)
    }
}

impl<F, N: NodePrimitives> OverlayStateProviderFactory<F, N, ExecutionOverlayOnly> {
    /// Creates a factory for execution overlays.
    pub fn new_execution(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self::with_kind(factory, overlay_builder)
    }
}

impl<F, N: NodePrimitives> OverlayStateProviderFactory<F, N, BothOverlays> {
    /// Creates a factory for execution and state trie overlays.
    pub fn new_both(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self::with_kind(factory, overlay_builder)
    }
}

impl<F, N, OverlayKind> OverlayStateProviderFactory<F, N, OverlayKind>
where
    N: NodePrimitives,
    OverlayKind: OverlayKindMarker,
{
    /// Fetches a [`StateTrieOverlay`] for known durable frontiers.
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn get_state_trie_overlay<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<StateTrieOverlay>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        let overlay = match self
            .state_trie_overlay_cache
            .entry((state_trie_tip_block.hash, finish_tip_block.hash))
        {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                self.metrics.state_trie_overlay_cache_misses.increment(1);
                let overlay = self.overlay_builder.build_state_trie_overlay_at_frontiers(
                    provider,
                    state_trie_tip_block,
                    finish_tip_block,
                )?;
                entry.insert(overlay.clone());
                overlay
            }
        };

        Ok(overlay)
    }

    /// Fetches an [`ExecutionOverlay`] for known durable frontiers.
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn get_execution_overlay<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<ExecutionOverlay>
    where
        Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader + PruneCheckpointReader,
    {
        let overlay = match self
            .execution_overlay_cache
            .entry((state_trie_tip_block.hash, finish_tip_block.hash))
        {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                self.metrics.execution_overlay_cache_misses.increment(1);
                let overlay = self.overlay_builder.build_execution_overlay_at_frontiers(
                    provider,
                    state_trie_tip_block,
                    finish_tip_block,
                )?;
                entry.insert(overlay.clone());
                overlay
            }
        };

        Ok(overlay)
    }

    fn get_overlays<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<(Option<StateTrieOverlay>, Option<ExecutionOverlay>)>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;
        let state_trie_overlay = OverlayKind::HAS_STATE_TRIE_OVERLAY
            .then(|| self.get_state_trie_overlay(provider, state_trie_tip_block, finish_tip_block))
            .transpose()?;
        let execution_overlay = OverlayKind::HAS_EXECUTION_OVERLAY
            .then(|| self.get_execution_overlay(provider, state_trie_tip_block, finish_tip_block))
            .transpose()?;
        Ok((state_trie_overlay, execution_overlay))
    }
}

impl<F, N, OverlayKind> DatabaseProviderROFactory for OverlayStateProviderFactory<F, N, OverlayKind>
where
    N: NodePrimitives,
    OverlayKind: OverlayKindMarker,
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
        + BlockNumReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache,
{
    type Provider = OverlayStateProvider<F::Provider, OverlayKind>;

    /// Create a read-only [`OverlayStateProvider`].
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn database_provider_ro(
        &self,
    ) -> ProviderResult<OverlayStateProvider<F::Provider, OverlayKind>> {
        let overall_start = Instant::now();

        // Get a read-only provider
        let provider = {
            let start = Instant::now();
            let res = self.factory.database_provider_ro()?;
            self.metrics.create_provider_duration.record(start.elapsed());
            res
        };

        let (state_trie_overlay, execution_overlay) = self.get_overlays(&provider)?;

        let is_v2 = provider.cached_storage_settings().is_v2();
        self.metrics.database_provider_ro_duration.record(overall_start.elapsed());
        Ok(OverlayStateProvider::from_overlays(
            provider,
            state_trie_overlay,
            execution_overlay,
            is_v2,
        ))
    }
}

/// Marker for a provider carrying only an execution overlay.
#[derive(Debug)]
pub struct ExecutionOverlayOnly;

/// Marker for a provider carrying only a state trie overlay.
#[derive(Debug)]
pub struct StateTrieOverlayOnly;

/// Marker for a provider carrying both overlays.
#[derive(Debug)]
pub struct BothOverlays;

/// Selects which overlays an [`OverlayStateProvider`] carries.
pub trait OverlayKindMarker {
    /// Whether providers of this kind carry a state trie overlay.
    const HAS_STATE_TRIE_OVERLAY: bool;
    /// Whether providers of this kind carry an execution overlay.
    const HAS_EXECUTION_OVERLAY: bool;
}

impl OverlayKindMarker for ExecutionOverlayOnly {
    const HAS_STATE_TRIE_OVERLAY: bool = false;
    const HAS_EXECUTION_OVERLAY: bool = true;
}

impl OverlayKindMarker for StateTrieOverlayOnly {
    const HAS_STATE_TRIE_OVERLAY: bool = true;
    const HAS_EXECUTION_OVERLAY: bool = false;
}

impl OverlayKindMarker for BothOverlays {
    const HAS_STATE_TRIE_OVERLAY: bool = true;
    const HAS_EXECUTION_OVERLAY: bool = true;
}

trait HasStateTrieOverlay: OverlayKindMarker {}

impl HasStateTrieOverlay for StateTrieOverlayOnly {}
impl HasStateTrieOverlay for BothOverlays {}

/// State provider with in-memory overlay from trie updates and hashed post state.
///
/// The marker type indicates whether this provider carries execution data, state trie data, or
/// both. Cursor factories are available only when it carries a state trie overlay.
pub struct OverlayStateProvider<Provider, OverlayKind = StateTrieOverlayOnly> {
    provider: Provider,
    state_trie_overlay: Option<StateTrieOverlay>,
    execution_overlay: Option<ExecutionOverlay>,
    is_v2: bool,
    _kind: PhantomData<OverlayKind>,
}

impl<Provider, OverlayKind> OverlayStateProvider<Provider, OverlayKind> {
    const fn from_overlays(
        provider: Provider,
        state_trie_overlay: Option<StateTrieOverlay>,
        execution_overlay: Option<ExecutionOverlay>,
        is_v2: bool,
    ) -> Self {
        Self { provider, state_trie_overlay, execution_overlay, is_v2, _kind: PhantomData }
    }
}

impl<Provider: fmt::Debug, OverlayKind> fmt::Debug for OverlayStateProvider<Provider, OverlayKind> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayStateProvider")
            .field("provider", &self.provider)
            .field("state_trie_overlay", &self.state_trie_overlay)
            .field("execution_overlay", &self.execution_overlay)
            .field("is_v2", &self.is_v2)
            .finish()
    }
}

impl<Provider> OverlayStateProvider<Provider, ExecutionOverlayOnly> {
    /// Creates an overlay state provider with execution data only.
    pub const fn new_execution(
        provider: Provider,
        execution_overlay: ExecutionOverlay,
        is_v2: bool,
    ) -> Self {
        Self {
            provider,
            state_trie_overlay: None,
            execution_overlay: Some(execution_overlay),
            is_v2,
            _kind: PhantomData,
        }
    }
}

impl<Provider> OverlayStateProvider<Provider, StateTrieOverlayOnly> {
    /// Creates an overlay state provider with state trie data only.
    pub const fn new_state_trie(
        provider: Provider,
        state_trie_overlay: StateTrieOverlay,
        is_v2: bool,
    ) -> Self {
        Self {
            provider,
            state_trie_overlay: Some(state_trie_overlay),
            execution_overlay: None,
            is_v2,
            _kind: PhantomData,
        }
    }
}

impl<Provider> OverlayStateProvider<Provider, BothOverlays> {
    /// Creates an overlay state provider with execution and state trie data.
    pub const fn new_both(
        provider: Provider,
        state_trie_overlay: StateTrieOverlay,
        execution_overlay: ExecutionOverlay,
        is_v2: bool,
    ) -> Self {
        Self {
            provider,
            state_trie_overlay: Some(state_trie_overlay),
            execution_overlay: Some(execution_overlay),
            is_v2,
            _kind: PhantomData,
        }
    }
}

impl<Provider, OverlayKind> TrieCursorFactory for OverlayStateProvider<Provider, OverlayKind>
where
    Provider: DbTxProvider,
    OverlayKind: HasStateTrieOverlay,
{
    type AccountTrieCursor<'a>
        = InMemoryTrieCursor<'a, Box<dyn TrieCursor + Send + 'a>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = InMemoryTrieCursor<'a, Box<dyn TrieStorageCursor + Send + 'a>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let overlay = self
            .state_trie_overlay
            .as_ref()
            .expect("state trie overlay kind must contain a state trie overlay");
        let cursor: Box<dyn TrieCursor + Send> = if self.is_v2 {
            Box::new(DatabaseAccountTrieCursor::<_, PackedKeyAdapter>::new(
                self.provider.tx().cursor_read::<PackedAccountsTrie>()?,
            ))
        } else {
            Box::new(DatabaseAccountTrieCursor::<_, LegacyKeyAdapter>::new(
                self.provider.tx().cursor_read::<tables::AccountsTrie>()?,
            ))
        };
        Ok(InMemoryTrieCursor::new_account(cursor, &overlay.trie_updates))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let overlay = self
            .state_trie_overlay
            .as_ref()
            .expect("state trie overlay kind must contain a state trie overlay");
        let cursor: Box<dyn TrieStorageCursor + Send> = if self.is_v2 {
            Box::new(DatabaseStorageTrieCursor::<_, PackedKeyAdapter>::new(
                self.provider.tx().cursor_dup_read::<PackedStoragesTrie>()?,
                hashed_address,
            ))
        } else {
            Box::new(DatabaseStorageTrieCursor::<_, LegacyKeyAdapter>::new(
                self.provider.tx().cursor_dup_read::<tables::StoragesTrie>()?,
                hashed_address,
            ))
        };
        Ok(InMemoryTrieCursor::new_storage(cursor, &overlay.trie_updates, hashed_address))
    }
}

impl<Provider, OverlayKind> HashedCursorFactory for OverlayStateProvider<Provider, OverlayKind>
where
    Provider: DbTxProvider,
    OverlayKind: HasStateTrieOverlay,
{
    type AccountCursor<'a>
        = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<&'a Provider::Tx>,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::AccountCursor<'a>
    where
        Self: 'a;

    type StorageCursor<'a>
        = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<&'a Provider::Tx>,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::StorageCursor<'a>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let overlay = self
            .state_trie_overlay
            .as_ref()
            .expect("state trie overlay kind must contain a state trie overlay");
        HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(self.provider.tx()),
            &overlay.hashed_post_state,
        )
        .hashed_account_cursor()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let overlay = self
            .state_trie_overlay
            .as_ref()
            .expect("state trie overlay kind must contain a state trie overlay");
        HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(self.provider.tx()),
            &overlay.hashed_post_state,
        )
        .hashed_storage_cursor(hashed_address)
    }
}

#[cfg(all(test, feature = "partial-persistence"))]
mod tests {
    use super::*;
    use crate::OverlayManager;
    use alloy_primitives::U256;
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{
        updates::TrieUpdatesSorted, BranchNodeCompact, ComputedTrieData, HashedPostState,
        HashedStorage, Nibbles,
    };

    fn with_unique_trie_data(
        block: &ExecutedBlock<EthPrimitives>,
        id: u8,
    ) -> ExecutedBlock<EthPrimitives> {
        let hashed_address = B256::with_last_byte(id);
        let hashed_slot = B256::with_last_byte(id.saturating_add(32));
        let hashed_state = HashedPostState::default()
            .with_accounts([(hashed_address, Some(Account::default()))])
            .with_storages([(
                hashed_address,
                HashedStorage::from_iter([(hashed_slot, U256::from(id))]),
            )])
            .into_sorted();
        let trie_updates = TrieUpdatesSorted::new(
            vec![(
                Nibbles::from_nibbles([id]),
                Some(BranchNodeCompact::new(0, 0, 0, vec![], None)),
            )],
            Default::default(),
        );

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(trie_updates)),
        )
    }

    fn test_blocks() -> Vec<ExecutedBlock<EthPrimitives>> {
        TestBlockBuilder::eth()
            .get_executed_blocks(0..5)
            .enumerate()
            .map(|(index, block)| with_unique_trie_data(&block, index as u8 + 1))
            .collect()
    }

    fn setup_frontiers(
        state_trie_tip_index: usize,
        finish_tip_index: usize,
    ) -> (ProviderFactory<MockNodeTypesWithDB>, Vec<ExecutedBlock<EthPrimitives>>) {
        let factory = create_test_provider_factory();
        let blocks = test_blocks();
        let provider_rw = factory.provider_rw().unwrap();
        for block in &blocks[..=finish_tip_index] {
            provider_rw.insert_block(block.recovered_block()).unwrap();
        }
        provider_rw
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(blocks[finish_tip_index].block_number())
                    .with_finish_stage_checkpoint(FinishCheckpoint {
                        partial_state_trie: Some(blocks[state_trie_tip_index].block_number()),
                    }),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        (factory, blocks)
    }

    fn account_keys(overlay: &StateTrieOverlay) -> Vec<B256> {
        overlay.hashed_post_state.accounts.iter().map(|(key, _)| *key).collect()
    }

    fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
        overlay.trie_updates.account_nodes_ref().iter().map(|(path, _)| *path).collect()
    }

    #[test]
    fn overlay_cache_is_keyed_by_both_durable_frontiers() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let overlay_factory = OverlayStateProviderFactory::new(
            factory.clone(),
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );

        let provider = factory.provider().unwrap();
        let first = overlay_factory.get_overlays(&provider).unwrap().0.unwrap();
        assert_eq!(account_keys(&first), vec![B256::with_last_byte(3), B256::with_last_byte(4)]);
        drop(provider);

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(blocks[3].block_number()).with_finish_stage_checkpoint(
                    FinishCheckpoint { partial_state_trie: Some(blocks[2].block_number()) },
                ),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let second = overlay_factory.get_overlays(&provider).unwrap().0.unwrap();
        assert_eq!(account_keys(&second), vec![B256::with_last_byte(4)]);
        assert_eq!(account_node_paths(&second), vec![Nibbles::from_nibbles([4])]);
        assert_eq!(overlay_factory.state_trie_overlay_cache.len(), 2);
    }

    #[test]
    fn execution_overlay_factory_skips_state_trie_overlay() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let overlay_factory = OverlayStateProviderFactory::new_execution(
            factory,
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );

        let provider = overlay_factory.database_provider_ro().unwrap();

        assert!(provider.state_trie_overlay.is_none());
        assert!(provider.execution_overlay.is_some());
        assert!(overlay_factory.state_trie_overlay_cache.is_empty());
        assert_eq!(overlay_factory.execution_overlay_cache.len(), 1);
    }

    #[test]
    fn combined_overlay_factory_builds_both_overlays() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let overlay_factory = OverlayStateProviderFactory::new_both(
            factory,
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );

        let provider = overlay_factory.database_provider_ro().unwrap();

        assert!(provider.state_trie_overlay.is_some());
        assert!(provider.execution_overlay.is_some());
        assert_eq!(overlay_factory.state_trie_overlay_cache.len(), 1);
        assert_eq!(overlay_factory.execution_overlay_cache.len(), 1);
    }
}
