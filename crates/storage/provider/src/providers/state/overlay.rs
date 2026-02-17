use alloy_primitives::{BlockNumber, B256};
use metrics::{Counter, Histogram};
use reth_chain_state::LazyOverlay;
use reth_db_api::{models::StorageLayout, DatabaseError};
use reth_errors::{ProviderError, ProviderResult};
use reth_metrics::Metrics;
use reth_primitives_traits::dashmap::{self, DashMap};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
    DatabaseProviderROFactory, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    HashedPostStateSorted,
};
use reth_trie_db::{ChangesetCache, DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::{
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
struct Overlay {
    trie_updates: Arc<TrieUpdatesSorted>,
    hashed_post_state: Arc<HashedPostStateSorted>,
}

/// Source of overlay data for [`OverlayStateProviderFactory`].
///
/// Either provides immediate pre-computed overlay data, or a lazy overlay that computes
/// on first access.
#[derive(Debug, Clone)]
pub enum OverlaySource {
    /// Immediate overlay with already-computed data.
    Immediate {
        /// Trie updates overlay.
        trie: Arc<TrieUpdatesSorted>,
        /// Hashed state overlay.
        state: Arc<HashedPostStateSorted>,
    },
    /// Lazy overlay computed on first access.
    Lazy(LazyOverlay),
}

impl OverlaySource {
    /// Resolve the overlay source into (trie, state) tuple.
    ///
    /// For lazy overlays, this may block waiting for deferred data.
    fn resolve(&self) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        match self {
            Self::Immediate { trie, state } => (Arc::clone(trie), Arc::clone(state)),
            Self::Lazy(lazy) => lazy.as_overlay(),
        }
    }
}

/// Factory for creating overlay state providers with optional reverts and overlays.
///
/// This factory allows building an `OverlayStateProvider` whose DB state has been reverted to a
/// particular block, and/or with additional overlay information added on top.
#[derive(Debug, Clone)]
pub struct OverlayStateProviderFactory<F> {
    /// The underlying database provider factory
    factory: F,
    /// Optional block hash for collecting reverts
    block_hash: Option<B256>,
    /// Optional overlay source (lazy or immediate).
    overlay_source: Option<OverlaySource>,
    /// Changeset cache handle for retrieving trie changesets
    changeset_cache: ChangesetCache,
    /// Metrics for tracking provider operations
    metrics: OverlayStateProviderMetrics,
    /// A cache which maps `db_tip -> Overlay`. If the db tip changes during usage of the factory
    /// then a new entry will get added to this, but in most cases only one entry is present.
    overlay_cache: Arc<DashMap<BlockNumber, Overlay>>,
}

impl<F> OverlayStateProviderFactory<F> {
    /// Create a new overlay state provider factory
    pub fn new(factory: F, changeset_cache: ChangesetCache) -> Self {
        Self {
            factory,
            block_hash: None,
            overlay_source: None,
            changeset_cache,
            metrics: OverlayStateProviderMetrics::default(),
            overlay_cache: Default::default(),
        }
    }

    /// Set the block hash for collecting reverts. All state will be reverted to the point
    /// _after_ this block has been processed.
    pub const fn with_block_hash(mut self, block_hash: Option<B256>) -> Self {
        self.block_hash = block_hash;
        self
    }

    /// Set the overlay source (lazy or immediate).
    ///
    /// This overlay will be applied on top of any reverts applied via `with_block_hash`.
    pub fn with_overlay_source(mut self, source: Option<OverlaySource>) -> Self {
        self.overlay_source = source;
        // Clear the overlay cache since we've updated the source.
        self.overlay_cache = Default::default();
        self
    }

    /// Set a lazy overlay that will be computed on first access.
    ///
    /// Convenience method that wraps the lazy overlay in `OverlaySource::Lazy`.
    pub fn with_lazy_overlay(mut self, lazy_overlay: Option<LazyOverlay>) -> Self {
        self.overlay_source = lazy_overlay.map(OverlaySource::Lazy);
        // Clear the overlay cache since we've updated the source.
        self.overlay_cache = Default::default();
        self
    }

    /// Set the hashed state overlay.
    ///
    /// This overlay will be applied on top of any reverts applied via `with_block_hash`.
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        if let Some(state) = hashed_state_overlay {
            self.overlay_source = Some(OverlaySource::Immediate {
                trie: Arc::new(TrieUpdatesSorted::default()),
                state,
            });
            // Clear the overlay cache since we've updated the source.
            self.overlay_cache = Default::default();
        }
        self
    }

    /// Extends the existing hashed state overlay with the given [`HashedPostStateSorted`].
    ///
    /// If no overlay exists, creates a new immediate overlay with the given state.
    /// If a lazy overlay exists, it is resolved first then extended.
    pub fn with_extended_hashed_state_overlay(mut self, other: HashedPostStateSorted) -> Self {
        match &mut self.overlay_source {
            Some(OverlaySource::Immediate { state, .. }) => {
                Arc::make_mut(state).extend_ref_and_sort(&other);
            }
            Some(OverlaySource::Lazy(lazy)) => {
                // Resolve lazy overlay and convert to immediate with extension
                let (trie, mut state) = lazy.as_overlay();
                Arc::make_mut(&mut state).extend_ref_and_sort(&other);
                self.overlay_source = Some(OverlaySource::Immediate { trie, state });
            }
            None => {
                self.overlay_source = Some(OverlaySource::Immediate {
                    trie: Arc::new(TrieUpdatesSorted::default()),
                    state: Arc::new(other),
                });
            }
        }
        // Clear the overlay cache since we've updated the source.
        self.overlay_cache = Default::default();
        self
    }
}

impl<F> OverlayStateProviderFactory<F>
where
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + DBProvider
        + BlockNumReader
        + StorageSettingsCache,
{
    /// Resolves the effective overlay (trie updates, hashed state).
    ///
    /// If an overlay source is set, it is resolved (blocking if lazy).
    /// Otherwise, returns empty defaults.
    fn resolve_overlays(&self) -> (Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>) {
        match &self.overlay_source {
            Some(source) => source.resolve(),
            None => {
                (Arc::new(TrieUpdatesSorted::default()), Arc::new(HashedPostStateSorted::default()))
            }
        }
    }

    /// Returns the block number for [`Self`]'s `block_hash` field, if any.
    fn get_requested_block_number(
        &self,
        provider: &F::Provider,
    ) -> ProviderResult<Option<BlockNumber>> {
        if let Some(block_hash) = self.block_hash {
            Ok(Some(
                provider
                    .convert_hash_or_number(block_hash.into())?
                    .ok_or_else(|| ProviderError::BlockHashNotFound(block_hash))?,
            ))
        } else {
            Ok(None)
        }
    }

    /// Returns the block which is at the tip of the DB, i.e. the block which the state tables of
    /// the DB are currently synced to.
    fn get_db_tip_block_number(&self, provider: &F::Provider) -> ProviderResult<BlockNumber> {
        provider
            .get_stage_checkpoint(StageId::Finish)?
            .as_ref()
            .map(|chk| chk.block_number)
            .ok_or_else(|| ProviderError::InsufficientChangesets { requested: 0, available: 0..=0 })
    }

    /// Returns whether or not it is required to collect reverts, and validates that there are
    /// sufficient changesets to revert to the requested block number if so.
    ///
    /// Takes into account both the stage checkpoint and the prune checkpoint to determine the
    /// available data range.
    fn reverts_required(
        &self,
        provider: &F::Provider,
        db_tip_block: BlockNumber,
        requested_block: BlockNumber,
    ) -> ProviderResult<bool> {
        // If the requested block is the DB tip then there won't be any reverts necessary, and we
        // can simply return Ok.
        if db_tip_block == requested_block {
            return Ok(false)
        }

        // Check account history prune checkpoint to determine the lower bound of available data.
        // The prune checkpoint's block_number is the highest pruned block, so data is available
        // starting from the next block.
        let prune_checkpoint = provider.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let lower_bound = prune_checkpoint
            .and_then(|chk| chk.block_number)
            .map(|block_number| block_number + 1)
            .unwrap_or_default();

        let available_range = lower_bound..=db_tip_block;

        // Check if the requested block is within the available range
        if !available_range.contains(&requested_block) {
            return Err(ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: available_range,
            });
        }

        Ok(true)
    }

    /// Calculates a new [`Overlay`] given a transaction and the current db tip.
    #[instrument(
        level = "debug",
        target = "providers::state::overlay",
        skip_all,
        fields(db_tip_block)
    )]
    fn calculate_overlay(
        &self,
        provider: &F::Provider,
        db_tip_block: BlockNumber,
    ) -> ProviderResult<Overlay> {
        //
        // Set up variables we'll use for recording metrics. There's two different code-paths here,
        // and we want to make sure both record metrics, so we do metrics recording after.
        let retrieve_trie_reverts_duration;
        let retrieve_hashed_state_reverts_duration;
        let trie_updates_total_len;
        let hashed_state_updates_total_len;

        // If block_hash is provided, collect reverts
        let (trie_updates, hashed_post_state) = if let Some(from_block) =
            self.get_requested_block_number(provider)? &&
            self.reverts_required(provider, db_tip_block, from_block)?
        {
            debug!(
                target: "providers::state::overlay",
                block_hash = ?self.block_hash,
                from_block,
                db_tip_block,
                range_start = from_block + 1,
                range_end = db_tip_block,
                "Collecting trie reverts for overlay state provider"
            );

            // Collect trie reverts using changeset cache
            let mut trie_reverts = {
                let _guard =
                    debug_span!(target: "providers::state::overlay", "Retrieving trie reverts")
                        .entered();

                let start = Instant::now();

                // Use changeset cache to retrieve and accumulate reverts to restore state after
                // from_block
                let accumulated_reverts = self
                    .changeset_cache
                    .get_or_compute_range(provider, (from_block + 1)..=db_tip_block)?;

                retrieve_trie_reverts_duration = start.elapsed();
                accumulated_reverts
            };

            // Collect state reverts
            let mut hashed_state_reverts = {
                let _guard = debug_span!(target: "providers::state::overlay", "Retrieving hashed state reverts").entered();

                let start = Instant::now();
                let res = reth_trie_db::from_reverts_auto(provider, from_block + 1..)?;
                retrieve_hashed_state_reverts_duration = start.elapsed();
                res
            };

            // Resolve overlays (lazy or immediate) and extend reverts with them.
            // If reverts are empty, use overlays directly to avoid cloning.
            let (overlay_trie, overlay_state) = self.resolve_overlays();

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
                block_hash = ?self.block_hash,
                ?from_block,
                num_trie_updates = ?trie_updates_total_len,
                num_state_updates = ?hashed_state_updates_total_len,
                "Reverted to target block",
            );

            (trie_updates, hashed_state_updates)
        } else {
            // If no block_hash, use overlays directly (resolving lazy if set)
            let (trie_updates, hashed_state) = self.resolve_overlays();

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

    /// Fetches an [`Overlay`] from the cache based on the current db tip block. If there is no
    /// cached value then this calculates the [`Overlay`] and populates the cache.
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn get_overlay(&self, provider: &F::Provider) -> ProviderResult<Overlay> {
        // If we have no anchor block configured then we will never need to get trie reverts, just
        // return the in-memory overlay (resolving lazy overlay if set).
        if self.block_hash.is_none() {
            let (trie_updates, hashed_post_state) = self.resolve_overlays();
            return Ok(Overlay { trie_updates, hashed_post_state })
        }

        let db_tip_block = self.get_db_tip_block_number(provider)?;

        // If the overlay is present in the cache then return it directly.
        if let Some(entry) = self.overlay_cache.get(&db_tip_block) {
            return Ok(entry.value().clone());
        }

        // If the overlay is not present then we need to calculate a new one.
        // DashMap's entry API handles the race condition internally.
        let mut cache_miss = false;
        let overlay = match self.overlay_cache.entry(db_tip_block) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                cache_miss = true;
                let overlay = self.calculate_overlay(provider, db_tip_block)?;
                entry.insert(overlay.clone());
                overlay
            }
        };

        if cache_miss {
            self.metrics.overlay_cache_misses.increment(1);
        }

        Ok(overlay)
    }
}

impl<F> DatabaseProviderROFactory for OverlayStateProviderFactory<F>
where
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
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
            let _guard =
                debug_span!(target: "providers::state::overlay", "Creating db provider").entered();

            let start = Instant::now();
            let res = self.factory.database_provider_ro()?;
            self.metrics.create_provider_duration.record(start.elapsed());
            res
        };

        let Overlay { trie_updates, hashed_post_state } = self.get_overlay(&provider)?;

        let layout = provider.cached_storage_settings().layout();

        self.metrics.database_provider_ro_duration.record(overall_start.elapsed());
        Ok(OverlayStateProvider::new(provider, trie_updates, hashed_post_state, layout))
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
    layout: StorageLayout,
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
        layout: StorageLayout,
    ) -> Self {
        Self { provider, trie_updates, hashed_post_state, layout }
    }
}

impl<Provider> TrieCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider,
{
    type AccountTrieCursor<'a>
        = <InMemoryTrieCursorFactory<
        DatabaseTrieCursorFactory<&'a Provider::Tx>,
        &'a TrieUpdatesSorted,
    > as TrieCursorFactory>::AccountTrieCursor<'a>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = <InMemoryTrieCursorFactory<
        DatabaseTrieCursorFactory<&'a Provider::Tx>,
        &'a TrieUpdatesSorted,
    > as TrieCursorFactory>::StorageTrieCursor<'a>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let db_trie_cursor_factory =
            DatabaseTrieCursorFactory::new(self.provider.tx_ref(), self.layout);
        let trie_cursor_factory =
            InMemoryTrieCursorFactory::new(db_trie_cursor_factory, self.trie_updates.as_ref());
        trie_cursor_factory.account_trie_cursor()
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let db_trie_cursor_factory =
            DatabaseTrieCursorFactory::new(self.provider.tx_ref(), self.layout);
        let trie_cursor_factory =
            InMemoryTrieCursorFactory::new(db_trie_cursor_factory, self.trie_updates.as_ref());
        trie_cursor_factory.storage_trie_cursor(hashed_address)
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
