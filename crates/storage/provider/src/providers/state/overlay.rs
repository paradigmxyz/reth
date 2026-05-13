use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber, B256};
use metrics::{Counter, Histogram};
use reth_chain_state::{EthPrimitives, StateTrieOverlay, StateTrieOverlayManager};
use reth_db_api::{tables, transaction::DbTx, DatabaseError};
use reth_errors::{ProviderError, ProviderResult};
use reth_metrics::Metrics;
use reth_primitives_traits::{
    dashmap::{self, DashMap},
    NodePrimitives,
};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
    DatabaseProviderROFactory, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursor, TrieCursor, TrieCursorFactory, TrieStorageCursor},
    updates::TrieUpdatesSorted,
    HashedPostStateSorted,
};
use reth_trie_db::{
    ChangesetCache, DatabaseAccountTrieCursor, DatabaseHashedCursorFactory,
    DatabaseStorageTrieCursor, LegacyKeyAdapter, PackedAccountsTrie, PackedKeyAdapter,
    PackedStoragesTrie,
};
use std::{
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, debug_span, instrument};

/// Metrics for overlay state provider operations.
#[derive(Clone, Metrics)]
#[metrics(scope = "storage.providers.overlay")]
pub(crate) struct OverlayStateProviderMetrics {
    /// Duration of creating the database provider transaction
    create_provider_duration: Histogram,
    /// Duration of retrieving trie updates from the database
    retrieve_trie_reverts_duration: Histogram,
    /// Duration of retrieving hashed state from the database
    retrieve_hashed_state_reverts_duration: Histogram,
    /// Size of trie updates (number of entries)
    trie_updates_size: Histogram,
    /// Size of hashed state (number of entries)
    hashed_state_size: Histogram,
    /// Overall duration of the [`OverlayStateProviderFactory::database_provider_ro`] call
    database_provider_ro_duration: Histogram,
    /// Number of cache misses when fetching [`Overlay`]s from the overlay cache.
    overlay_cache_misses: Counter,
}

/// Contains all fields required to initialize an [`OverlayStateProvider`].
#[derive(Debug, Clone)]
pub(super) struct Overlay {
    pub(super) trie_updates: Arc<TrieUpdatesSorted>,
    pub(super) hashed_post_state: Arc<HashedPostStateSorted>,
}

/// Source of overlay data for [`OverlayStateProviderFactory`].
#[derive(Debug, Clone)]
pub(super) enum OverlaySource<N: NodePrimitives = EthPrimitives> {
    /// Immediate overlay with already-computed data.
    Immediate {
        /// Trie updates overlay.
        trie: Arc<TrieUpdatesSorted>,
        /// Hashed state overlay.
        state: Arc<HashedPostStateSorted>,
    },
    /// Manager-backed overlay for in-memory state, with optional immediate overlay data.
    Managed {
        /// Manager used to resolve in-memory parent state if the target is not persisted.
        manager: StateTrieOverlayManager<N>,
        /// Trie updates overlay.
        trie: Arc<TrieUpdatesSorted>,
        /// Hashed state overlay.
        state: Arc<HashedPostStateSorted>,
    },
}

impl<N: NodePrimitives> OverlaySource<N> {
    const fn immediate_overlay(&self) -> (&Arc<TrieUpdatesSorted>, &Arc<HashedPostStateSorted>) {
        match self {
            Self::Immediate { trie, state } | Self::Managed { trie, state, .. } => (trie, state),
        }
    }

    fn into_immediate_overlay(self) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        match self {
            Self::Immediate { trie, state } | Self::Managed { trie, state, .. } => (trie, state),
        }
    }
}

/// Builder for calculating trie and hashed-state overlays.
///
/// This stores the overlay configuration and the logic for resolving immediate/lazy overlays and
/// collecting reverts. It is intentionally independent from any provider factory or overlay cache.
#[derive(Debug, Clone)]
pub struct OverlayBuilder<N: NodePrimitives = EthPrimitives> {
    /// Target state hash requested by the caller.
    target_hash: B256,
    /// Optional overlay source.
    overlay_source: Option<OverlaySource<N>>,
    /// Changeset cache handle for retrieving trie changesets
    changeset_cache: ChangesetCache,
    /// Metrics for tracking provider operations
    metrics: OverlayStateProviderMetrics,
}

impl<N: NodePrimitives> OverlayBuilder<N> {
    /// Create a new overlay builder.
    pub fn new(target_hash: B256, changeset_cache: ChangesetCache) -> Self {
        Self {
            target_hash,
            overlay_source: None,
            changeset_cache,
            metrics: OverlayStateProviderMetrics::default(),
        }
    }

    /// Set the overlay source.
    ///
    /// This overlay will be applied on top of any reverts and managed overlay data.
    pub(super) fn with_overlay_source(mut self, source: Option<OverlaySource<N>>) -> Self {
        self.overlay_source = source;
        self
    }

    /// Set the state trie overlay manager used to resolve in-memory parent state.
    pub fn with_state_trie_overlay_manager(
        mut self,
        state_trie_overlay_manager: StateTrieOverlayManager<N>,
    ) -> Self {
        let (trie, state) = self.overlay_source.take().map_or_else(
            || (Arc::new(TrieUpdatesSorted::default()), Arc::new(HashedPostStateSorted::default())),
            |source| source.into_immediate_overlay(),
        );

        self.overlay_source =
            Some(OverlaySource::Managed { manager: state_trie_overlay_manager, trie, state });
        self
    }

    /// Set the hashed state overlay.
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        if let Some(state) = hashed_state_overlay {
            match &mut self.overlay_source {
                Some(OverlaySource::Managed { state: managed_state, .. }) => {
                    *managed_state = state;
                }
                _ => {
                    self.overlay_source = Some(OverlaySource::Immediate {
                        trie: Arc::new(TrieUpdatesSorted::default()),
                        state,
                    });
                }
            }
        }
        self
    }

    /// Extends the existing hashed state overlay with the given [`HashedPostStateSorted`].
    ///
    /// If no immediate overlay exists, creates one with the given state.
    pub fn with_extended_hashed_state_overlay(mut self, other: HashedPostStateSorted) -> Self {
        match &mut self.overlay_source {
            Some(OverlaySource::Immediate { state, .. } | OverlaySource::Managed { state, .. }) => {
                Arc::make_mut(state).extend_ref_and_sort(&other);
            }
            None => {
                self.overlay_source = Some(OverlaySource::Immediate {
                    trie: Arc::new(TrieUpdatesSorted::default()),
                    state: Arc::new(other),
                });
            }
        }
        self
    }

    /// Resolves the effective overlay (trie updates, hashed state).
    fn resolve_overlays(
        &self,
        anchor_hash: BlockHash,
        state_trie_overlay: Option<&StateTrieOverlay<N>>,
    ) -> ProviderResult<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>)> {
        let (mut trie_updates, mut hashed_post_state) =
            if let Some(state_trie_overlay) = state_trie_overlay {
                state_trie_overlay.as_overlay(anchor_hash).map_err(ProviderError::other)?
            } else {
                (Arc::new(TrieUpdatesSorted::default()), Arc::new(HashedPostStateSorted::default()))
            };

        if let Some(source) = &self.overlay_source {
            let (trie, state) = source.immediate_overlay();
            if trie_updates.is_empty() {
                trie_updates = Arc::clone(trie);
            } else if !trie.is_empty() {
                Arc::make_mut(&mut trie_updates).extend_ref_and_sort(trie);
            }
            if hashed_post_state.is_empty() {
                hashed_post_state = Arc::clone(state);
            } else if !state.is_empty() {
                Arc::make_mut(&mut hashed_post_state).extend_ref_and_sort(state);
            }
        }

        Ok((trie_updates, hashed_post_state))
    }

    /// Resolves the in-memory state trie overlay needed for the configured target hash.
    fn resolve_state_trie_overlay<Provider>(
        &self,
        provider: &Provider,
        db_tip_block: BlockNumHash,
    ) -> ProviderResult<ResolvedStateTrieOverlay<N>>
    where
        Provider: BlockNumReader,
    {
        if let Some(target_number) = provider.convert_hash_or_number(self.target_hash.into())? &&
            target_number <= db_tip_block.number
        {
            debug!(
                target: "providers::state::overlay",
                target_hash = %self.target_hash,
                "Target found in persisted DB state, no state trie overlay needed"
            );
            return Ok(ResolvedStateTrieOverlay { anchor_hash: self.target_hash, overlay: None })
        }

        let Some(OverlaySource::Managed { manager, .. }) = &self.overlay_source else {
            return Ok(ResolvedStateTrieOverlay { anchor_hash: self.target_hash, overlay: None })
        };

        let (overlay, anchor_hash) =
            manager.overlay_for_parent(self.target_hash).map_err(ProviderError::other)?;
        debug!(
            target: "providers::state::overlay",
            target_hash = %self.target_hash,
            %anchor_hash,
            "Using managed state trie overlay"
        );

        Ok(ResolvedStateTrieOverlay { anchor_hash, overlay: Some(overlay) })
    }

    /// Returns the block number for `anchor_hash`.
    fn get_block_number<Provider>(
        &self,
        provider: &Provider,
        anchor_hash: B256,
    ) -> ProviderResult<BlockNumber>
    where
        Provider: BlockNumReader,
    {
        provider
            .convert_hash_or_number(anchor_hash.into())?
            .ok_or(ProviderError::BlockHashNotFound(anchor_hash))
    }

    /// Returns the block which is at the tip of the DB, i.e. the block which the state tables of
    /// the DB are currently synced to.
    fn get_db_tip_block<Provider>(&self, provider: &Provider) -> ProviderResult<BlockNumHash>
    where
        Provider: StageCheckpointReader + BlockNumReader,
    {
        let block_number = provider
            .get_stage_checkpoint(StageId::Finish)?
            .as_ref()
            .map(|chk| chk.block_number)
            .ok_or_else(|| ProviderError::InsufficientChangesets {
                requested: 0,
                available: 0..=0,
            })?;
        let hash = provider
            .convert_number(block_number.into())?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        Ok(BlockNumHash::new(block_number, hash))
    }

    /// Returns whether or not it is required to collect reverts, and validates that there are
    /// sufficient changesets to revert to the requested block number if so.
    ///
    /// Takes into account both the stage checkpoint and the prune checkpoint to determine the
    /// available data range.
    fn reverts_required<Provider>(
        &self,
        provider: &Provider,
        db_tip_block: BlockNumHash,
        resolved: &ResolvedStateTrieOverlay<N>,
    ) -> ProviderResult<Option<RangeInclusive<BlockNumber>>>
    where
        Provider: BlockNumReader + PruneCheckpointReader,
    {
        // If the anchor is the DB tip then there won't be any reverts necessary.
        if db_tip_block.hash == resolved.anchor_hash {
            return Ok(None)
        }

        // If the DB tip has moved forward into the managed overlay then we still don't need to
        // revert; the overlay will generate a new in-memory view using only the relevant
        // blocks data.
        if let Some(state_trie_overlay) = &resolved.overlay &&
            state_trie_overlay.has_anchor_hash(db_tip_block.hash)
        {
            return Ok(None)
        }

        let anchor_number = self.get_block_number(provider, resolved.anchor_hash)?;

        // Check account history prune checkpoint to determine the lower bound of available data.
        // The prune checkpoint's block_number is the highest pruned block, so data is available
        // starting from the next block.
        let prune_checkpoint = provider.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let lower_bound = prune_checkpoint
            .and_then(|chk| chk.block_number)
            .map(|block_number| block_number + 1)
            .unwrap_or_default();

        let available_range = lower_bound..=db_tip_block.number;

        // Check if the requested block is within the available range
        if !available_range.contains(&anchor_number) {
            return Err(ProviderError::InsufficientChangesets {
                requested: anchor_number,
                available: available_range,
            });
        }

        Ok(Some(anchor_number + 1..=db_tip_block.number))
    }

    /// Calculates a new [`Overlay`] given a transaction and the current db tip.
    #[instrument(
        level = "debug",
        target = "providers::state::overlay",
        skip_all,
        fields(?db_tip_block, target_hash = ?self.target_hash)
    )]
    fn calculate_overlay<Provider>(
        &self,
        provider: &Provider,
        db_tip_block: BlockNumHash,
        resolved: &ResolvedStateTrieOverlay<N>,
    ) -> ProviderResult<Overlay>
    where
        Provider: ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + StorageSettingsCache,
    {
        //
        // Set up variables we'll use for recording metrics. There's two different code-paths here,
        // and we want to make sure both record metrics, so we do metrics recording after.
        let retrieve_trie_reverts_duration;
        let retrieve_hashed_state_reverts_duration;
        let trie_updates_total_len;
        let hashed_state_updates_total_len;

        // Collect any reverts which are required to bring the DB view back to the anchor hash.
        let (trie_updates, hashed_post_state) = if let Some(revert_blocks) =
            self.reverts_required(provider, db_tip_block, resolved)?
        {
            debug!(
                target: "providers::state::overlay",
                ?revert_blocks,
                "Collecting trie reverts for overlay state provider"
            );

            // Collect trie reverts using changeset cache
            let mut trie_reverts = {
                let _guard =
                    debug_span!(target: "providers::state::overlay", "retrieving_trie_reverts")
                        .entered();

                let start = Instant::now();

                // Use changeset cache to retrieve and accumulate reverts to restore state after
                // from_block
                let accumulated_reverts =
                    self.changeset_cache.get_or_compute_range(provider, revert_blocks.clone())?;

                retrieve_trie_reverts_duration = start.elapsed();
                accumulated_reverts
            };

            // Collect state reverts
            let mut hashed_state_reverts = {
                let _guard = debug_span!(target: "providers::state::overlay", "retrieving_hashed_state_reverts").entered();

                let start = Instant::now();
                let res = reth_trie_db::from_reverts_auto(provider, revert_blocks)?;
                retrieve_hashed_state_reverts_duration = start.elapsed();
                res
            };

            // Resolve overlays (lazy or immediate) and extend reverts with them.
            // If reverts are empty, use overlays directly to avoid cloning.
            let (overlay_trie, overlay_state) =
                self.resolve_overlays(resolved.anchor_hash, resolved.overlay.as_ref())?;

            let trie_updates = if trie_reverts.is_empty() {
                overlay_trie
            } else if !overlay_trie.is_empty() {
                trie_reverts.extend_ref_and_sort(&overlay_trie);
                Arc::new(trie_reverts)
            } else {
                Arc::new(trie_reverts)
            };

            let hashed_state_updates = if hashed_state_reverts.is_empty() {
                overlay_state
            } else if !overlay_state.is_empty() {
                hashed_state_reverts.extend_ref_and_sort(&overlay_state);
                Arc::new(hashed_state_reverts)
            } else {
                Arc::new(hashed_state_reverts)
            };

            trie_updates_total_len = trie_updates.total_len();
            hashed_state_updates_total_len = hashed_state_updates.total_len();

            debug!(
                target: "providers::state::overlay",
                num_trie_updates = ?trie_updates_total_len,
                num_state_updates = ?hashed_state_updates_total_len,
                "Reverted to target block",
            );

            (trie_updates, hashed_state_updates)
        } else {
            // If no reverts are needed then we can assume that the db tip is the anchor hash or
            // overlaps with the managed overlay. Use overlays directly.
            let (trie_updates, hashed_state) =
                self.resolve_overlays(db_tip_block.hash, resolved.overlay.as_ref())?;

            retrieve_trie_reverts_duration = Duration::ZERO;
            retrieve_hashed_state_reverts_duration = Duration::ZERO;
            trie_updates_total_len = trie_updates.total_len();
            hashed_state_updates_total_len = hashed_state.total_len();

            (trie_updates, hashed_state)
        };

        // Record metrics
        self.metrics
            .retrieve_trie_reverts_duration
            .record(retrieve_trie_reverts_duration.as_secs_f64());
        self.metrics
            .retrieve_hashed_state_reverts_duration
            .record(retrieve_hashed_state_reverts_duration.as_secs_f64());
        self.metrics.trie_updates_size.record(trie_updates_total_len as f64);
        self.metrics.hashed_state_size.record(hashed_state_updates_total_len as f64);

        Ok(Overlay { trie_updates, hashed_post_state })
    }

    /// Builds the effective overlay for the given provider.
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    pub(super) fn build_overlay<Provider>(&self, provider: &Provider) -> ProviderResult<Overlay>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        let db_tip_block = self.get_db_tip_block(provider)?;
        let resolved = self.resolve_state_trie_overlay(provider, db_tip_block)?;
        self.calculate_overlay(provider, db_tip_block, &resolved)
    }
}

#[derive(Debug)]
struct ResolvedStateTrieOverlay<N: NodePrimitives = EthPrimitives> {
    anchor_hash: B256,
    overlay: Option<StateTrieOverlay<N>>,
}

/// Factory for creating overlay state providers with optional reverts and overlays.
///
/// This factory allows building an `OverlayStateProvider` whose DB state has been reverted to a
/// particular block, and/or with additional overlay information added on top.
#[derive(Debug, Clone)]
pub struct OverlayStateProviderFactory<F, N: NodePrimitives = EthPrimitives> {
    /// The underlying database provider factory
    factory: F,
    /// Overlay builder containing the configuration and overlay calculation logic.
    overlay_builder: OverlayBuilder<N>,
    /// A cache which maps `db_tip -> Overlay`. If the db tip changes during usage of the factory
    /// then a new entry will get added to this, but in most cases only one entry is present.
    overlay_cache: Arc<DashMap<BlockHash, Overlay>>,
}

impl<F, N: NodePrimitives> OverlayStateProviderFactory<F, N> {
    /// Create a new overlay state provider factory
    pub fn new(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self { factory, overlay_builder, overlay_cache: Default::default() }
    }

    /// Set the hashed state overlay.
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        self.overlay_builder = self.overlay_builder.with_hashed_state_overlay(hashed_state_overlay);
        self.overlay_cache = Default::default();
        self
    }

    /// Extends the existing hashed state overlay with the given [`HashedPostStateSorted`].
    pub fn with_extended_hashed_state_overlay(mut self, other: HashedPostStateSorted) -> Self {
        self.overlay_builder = self.overlay_builder.with_extended_hashed_state_overlay(other);
        self.overlay_cache = Default::default();
        self
    }

    /// Fetches an [`Overlay`] from the cache based on the current db tip block. If there is no
    /// cached value then this calculates the [`Overlay`] and populates the cache.
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn get_overlay<Provider>(&self, provider: &Provider) -> ProviderResult<Overlay>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        let db_tip_block = self.overlay_builder.get_db_tip_block(provider)?;

        let overlay = match self.overlay_cache.entry(db_tip_block.hash) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                self.overlay_builder.metrics.overlay_cache_misses.increment(1);
                let overlay = self.overlay_builder.build_overlay(provider)?;
                entry.insert(overlay.clone());
                overlay
            }
        };

        Ok(overlay)
    }
}

impl<F, N> DatabaseProviderROFactory for OverlayStateProviderFactory<F, N>
where
    N: NodePrimitives,
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
        + DBProvider
        + BlockNumReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache,
{
    type Provider = OverlayStateProvider<F::Provider>;

    /// Create a read-only [`OverlayStateProvider`].
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn database_provider_ro(&self) -> ProviderResult<OverlayStateProvider<F::Provider>> {
        let overall_start = Instant::now();

        // Get a read-only provider
        let provider = {
            let start = Instant::now();
            let res = self.factory.database_provider_ro()?;
            self.overlay_builder.metrics.create_provider_duration.record(start.elapsed());
            res
        };

        let Overlay { trie_updates, hashed_post_state } = self.get_overlay(&provider)?;

        let is_v2 = provider.cached_storage_settings().is_v2();
        self.overlay_builder.metrics.database_provider_ro_duration.record(overall_start.elapsed());
        Ok(OverlayStateProvider::new(provider, trie_updates, hashed_post_state, is_v2))
    }
}

/// State provider with in-memory overlay from trie updates and hashed post state.
///
/// This provider uses in-memory trie updates and hashed post state as an overlay
/// on top of a database provider, implementing [`TrieCursorFactory`] and [`HashedCursorFactory`]
/// using the in-memory overlay factories.
#[derive(Debug)]
pub struct OverlayStateProvider<Provider: DBProvider> {
    provider: Provider,
    trie_updates: Arc<TrieUpdatesSorted>,
    hashed_post_state: Arc<HashedPostStateSorted>,
    is_v2: bool,
}

impl<Provider> OverlayStateProvider<Provider>
where
    Provider: DBProvider,
{
    /// Create new overlay state provider. The `Provider` must be cloneable, which generally means
    /// it should be wrapped in an `Arc`.
    pub const fn new(
        provider: Provider,
        trie_updates: Arc<TrieUpdatesSorted>,
        hashed_post_state: Arc<HashedPostStateSorted>,
        is_v2: bool,
    ) -> Self {
        Self { provider, trie_updates, hashed_post_state, is_v2 }
    }
}

impl<Provider> TrieCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider,
    Provider::Tx: DbTx,
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
        let tx = self.provider.tx_ref();
        let trie_updates = self.trie_updates.as_ref();
        let cursor: Box<dyn TrieCursor + Send> = if self.is_v2 {
            Box::new(DatabaseAccountTrieCursor::<_, PackedKeyAdapter>::new(
                tx.cursor_read::<PackedAccountsTrie>()?,
            ))
        } else {
            Box::new(DatabaseAccountTrieCursor::<_, LegacyKeyAdapter>::new(
                tx.cursor_read::<tables::AccountsTrie>()?,
            ))
        };
        Ok(InMemoryTrieCursor::new_account(cursor, trie_updates))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let tx = self.provider.tx_ref();
        let trie_updates = self.trie_updates.as_ref();
        let cursor: Box<dyn TrieStorageCursor + Send> = if self.is_v2 {
            Box::new(DatabaseStorageTrieCursor::<_, PackedKeyAdapter>::new(
                tx.cursor_dup_read::<PackedStoragesTrie>()?,
                hashed_address,
            ))
        } else {
            Box::new(DatabaseStorageTrieCursor::<_, LegacyKeyAdapter>::new(
                tx.cursor_dup_read::<tables::StoragesTrie>()?,
                hashed_address,
            ))
        };
        Ok(InMemoryTrieCursor::new_storage(cursor, trie_updates, hashed_address))
    }
}

impl<Provider> HashedCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider,
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
        let db_hashed_cursor_factory = DatabaseHashedCursorFactory::new(self.provider.tx_ref());
        let hashed_cursor_factory =
            HashedPostStateCursorFactory::new(db_hashed_cursor_factory, &self.hashed_post_state);
        hashed_cursor_factory.hashed_account_cursor()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let db_hashed_cursor_factory = DatabaseHashedCursorFactory::new(self.provider.tx_ref());
        let hashed_cursor_factory =
            HashedPostStateCursorFactory::new(db_hashed_cursor_factory, &self.hashed_post_state);
        hashed_cursor_factory.hashed_storage_cursor(hashed_address)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chainspec::ChainInfo;
    use reth_primitives_traits::Account;
    use reth_storage_api::BlockHashReader;
    use reth_trie::HashedPostState;
    use std::collections::HashMap;

    #[derive(Default)]
    struct TestBlockProvider {
        tip: BlockNumHash,
        numbers: HashMap<B256, BlockNumber>,
        hashes: HashMap<BlockNumber, B256>,
    }

    impl TestBlockProvider {
        fn new(tip: BlockNumHash) -> Self {
            Self { tip, ..Default::default() }
        }

        fn with_block(mut self, hash: B256, number: BlockNumber) -> Self {
            self.numbers.insert(hash, number);
            self.hashes.insert(number, hash);
            self
        }
    }

    impl BlockHashReader for TestBlockProvider {
        fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
            Ok(self.hashes.get(&number).copied())
        }

        fn canonical_hashes_range(
            &self,
            start: BlockNumber,
            end: BlockNumber,
        ) -> ProviderResult<Vec<B256>> {
            Ok((start..end).filter_map(|number| self.hashes.get(&number).copied()).collect())
        }
    }

    impl BlockNumReader for TestBlockProvider {
        fn chain_info(&self) -> ProviderResult<ChainInfo> {
            Ok(ChainInfo { best_hash: self.tip.hash, best_number: self.tip.number })
        }

        fn best_block_number(&self) -> ProviderResult<BlockNumber> {
            Ok(self.tip.number)
        }

        fn last_block_number(&self) -> ProviderResult<BlockNumber> {
            Ok(self.tip.number)
        }

        fn block_number(&self, hash: B256) -> ProviderResult<Option<BlockNumber>> {
            Ok(self.numbers.get(&hash).copied())
        }
    }

    #[test]
    fn resolved_overlay_skips_manager_for_persisted_parent() {
        let parent_hash = B256::with_last_byte(1);
        let db_tip = BlockNumHash::new(10, B256::with_last_byte(2));
        let provider = TestBlockProvider::new(db_tip).with_block(parent_hash, 9);
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default());

        let resolved = builder.resolve_state_trie_overlay(&provider, db_tip).unwrap();

        assert_eq!(resolved.anchor_hash, parent_hash);
        assert!(resolved.overlay.is_none());
    }

    #[test]
    fn resolved_overlay_errors_if_parent_is_not_persisted_or_managed() {
        let parent_hash = B256::with_last_byte(1);
        let db_tip = BlockNumHash::new(10, B256::with_last_byte(2));
        let provider = TestBlockProvider::new(db_tip).with_block(parent_hash, 11);
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default());

        let err = builder.resolve_state_trie_overlay(&provider, db_tip).unwrap_err();

        assert!(err.to_string().contains("not tracked by the manager"));
    }

    #[test]
    fn extending_hashed_state_keeps_managed_overlay_source() {
        let target_hash = B256::with_last_byte(1);
        let hashed_state = HashedPostState::default()
            .with_accounts([(B256::with_last_byte(2), Some(Account::default()))])
            .into_sorted();
        let builder = OverlayBuilder::<EthPrimitives>::new(target_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default())
            .with_extended_hashed_state_overlay(hashed_state);

        let Some(OverlaySource::Managed { state, .. }) = builder.overlay_source else {
            panic!("expected managed overlay source")
        };
        assert_eq!(state.total_len(), 1);
    }
}
