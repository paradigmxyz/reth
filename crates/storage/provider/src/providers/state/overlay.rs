use alloy_primitives::{BlockNumber, B256};
use metrics::Histogram;
use reth_db_api::DatabaseError;
use reth_errors::{ProviderError, ProviderResult};
use reth_metrics::Metrics;
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockNumReader, DBProvider, DatabaseProviderFactory, DatabaseProviderROFactory,
    PruneCheckpointReader, StageCheckpointReader, TrieReader,
};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    HashedPostState, HashedPostStateSorted, KeccakKeyHasher,
};
use reth_trie_db::{
    DatabaseHashedCursorFactory, DatabaseHashedPostState, DatabaseTrieCursorFactory,
};
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
    /// Optional trie overlay
    trie_overlay: Option<Arc<TrieUpdatesSorted>>,
    /// Optional hashed state overlay
    hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    /// Metrics for tracking provider operations
    metrics: OverlayStateProviderMetrics,
}

impl<F> OverlayStateProviderFactory<F> {
    /// Create a new overlay state provider factory
    pub fn new(factory: F) -> Self {
        Self {
            factory,
            block_hash: None,
            trie_overlay: None,
            hashed_state_overlay: None,
            metrics: OverlayStateProviderMetrics::default(),
        }
    }

    /// Set the block hash for collecting reverts. All state will be reverted to the point
    /// _after_ this block has been processed.
    pub const fn with_block_hash(mut self, block_hash: Option<B256>) -> Self {
        self.block_hash = block_hash;
        self
    }

    /// Set the trie overlay.
    ///
    /// This overlay will be applied on top of any reverts applied via `with_block_hash`.
    pub fn with_trie_overlay(mut self, trie_overlay: Option<Arc<TrieUpdatesSorted>>) -> Self {
        self.trie_overlay = trie_overlay;
        self
    }

    /// Set the hashed state overlay
    ///
    /// This overlay will be applied on top of any reverts applied via `with_block_hash`.
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        self.hashed_state_overlay = hashed_state_overlay;
        self
    }
}

impl<F> OverlayStateProviderFactory<F>
where
    F: DatabaseProviderFactory,
    F::Provider: TrieReader + StageCheckpointReader + PruneCheckpointReader + BlockNumReader,
{
    /// Returns the block number for [`Self`]'s `block_hash` field, if any.
    fn get_block_number(&self, provider: &F::Provider) -> ProviderResult<Option<BlockNumber>> {
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

    /// Returns whether or not it is required to collect reverts, and validates that there are
    /// sufficient changesets to revert to the requested block number if so.
    ///
    /// Returns an error if the `MerkleChangeSets` checkpoint doesn't cover the requested block.
    /// Takes into account both the stage checkpoint and the prune checkpoint to determine the
    /// available data range.
    fn reverts_required(
        &self,
        provider: &F::Provider,
        requested_block: BlockNumber,
    ) -> ProviderResult<bool> {
        // Get the MerkleChangeSets stage and prune checkpoints.
        let stage_checkpoint = provider.get_stage_checkpoint(StageId::MerkleChangeSets)?;
        let prune_checkpoint = provider.get_prune_checkpoint(PruneSegment::MerkleChangeSets)?;

        // Get the upper bound from stage checkpoint
        let upper_bound =
            stage_checkpoint.as_ref().map(|chk| chk.block_number).ok_or_else(|| {
                ProviderError::InsufficientChangesets {
                    requested: requested_block,
                    available: 0..=0,
                }
            })?;

        // If the requested block is the DB tip (determined by the MerkleChangeSets stage
        // checkpoint) then there won't be any reverts necessary, and we can simply return Ok.
        if upper_bound == requested_block {
            return Ok(false)
        }

        // Extract the lower bound from prune checkpoint if available.
        //
        // If not available we assume pruning has never ran and so there is no lower bound. This
        // should not generally happen, since MerkleChangeSets always have pruning enabled, but when
        // starting a new node from scratch (e.g. in a test case or benchmark) it can surface.
        //
        // The prune checkpoint's block_number is the highest pruned block, so data is available
        // starting from the next block
        let lower_bound = prune_checkpoint
            .and_then(|chk| chk.block_number)
            .map(|block_number| block_number + 1)
            .unwrap_or_default();

        let available_range = lower_bound..=upper_bound;

        // Check if the requested block is within the available range
        if !available_range.contains(&requested_block) {
            return Err(ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: available_range,
            });
        }

        Ok(true)
    }
}

impl<F> DatabaseProviderROFactory for OverlayStateProviderFactory<F>
where
    F: DatabaseProviderFactory,
    F::Provider: TrieReader + StageCheckpointReader + PruneCheckpointReader + BlockNumReader,
{
    type Provider = OverlayStateProvider<F::Provider>;

    /// Create a read-only [`OverlayStateProvider`].
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn database_provider_ro(&self) -> ProviderResult<OverlayStateProvider<F::Provider>> {
        // Get a read-only provider
        let provider = {
            let _guard =
                debug_span!(target: "providers::state::overlay", "Creating db provider").entered();

            let start = Instant::now();
            let res = self.factory.database_provider_ro()?;
            self.metrics.create_provider_duration.record(start.elapsed());
            res
        };

        // Set up variables we'll use for recording metrics. There's two different code-paths here,
        // and we want to make sure both record metrics, so we do metrics recording after.
        let retrieve_trie_reverts_duration;
        let retrieve_hashed_state_reverts_duration;
        let trie_updates_total_len;
        let hashed_state_updates_total_len;

        // If block_hash is provided, collect reverts
        let (trie_updates, hashed_state) = if let Some(from_block) =
            self.get_block_number(&provider)? &&
            self.reverts_required(&provider, from_block)?
        {
            // Collect trie reverts
            let mut trie_reverts = {
                let _guard =
                    debug_span!(target: "providers::state::overlay", "Retrieving trie reverts")
                        .entered();

                let start = Instant::now();
                let res = provider.trie_reverts(from_block + 1)?;
                retrieve_trie_reverts_duration = start.elapsed();
                res
            };

            // Collect state reverts
            let mut hashed_state_reverts = {
                let _guard = debug_span!(target: "providers::state::overlay", "Retrieving hashed state reverts").entered();

                let start = Instant::now();
                // TODO(mediocregopher) make from_reverts return sorted
                // https://github.com/paradigmxyz/reth/issues/19382
                let res = HashedPostState::from_reverts::<KeccakKeyHasher>(
                    provider.tx_ref(),
                    from_block + 1..,
                )?
                .into_sorted();
                retrieve_hashed_state_reverts_duration = start.elapsed();
                res
            };

            // Extend with overlays if provided. If the reverts are empty we should just use the
            // overlays directly, because `extend_ref` will actually clone the overlay.
            let trie_updates = match self.trie_overlay.as_ref() {
                Some(trie_overlay) if trie_reverts.is_empty() => Arc::clone(trie_overlay),
                Some(trie_overlay) => {
                    trie_reverts.extend_ref(trie_overlay);
                    Arc::new(trie_reverts)
                }
                None => Arc::new(trie_reverts),
            };

            let hashed_state_updates = match self.hashed_state_overlay.as_ref() {
                Some(hashed_state_overlay) if hashed_state_reverts.is_empty() => {
                    Arc::clone(hashed_state_overlay)
                }
                Some(hashed_state_overlay) => {
                    hashed_state_reverts.extend_ref(hashed_state_overlay);
                    Arc::new(hashed_state_reverts)
                }
                None => Arc::new(hashed_state_reverts),
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
            // If no block_hash, use overlays directly or defaults
            let trie_updates =
                self.trie_overlay.clone().unwrap_or_else(|| Arc::new(TrieUpdatesSorted::default()));
            let hashed_state = self
                .hashed_state_overlay
                .clone()
                .unwrap_or_else(|| Arc::new(HashedPostStateSorted::default()));

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

        Ok(OverlayStateProvider::new(provider, trie_updates, hashed_state))
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
    ) -> Self {
        Self { provider, trie_updates, hashed_post_state }
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
        let db_trie_cursor_factory = DatabaseTrieCursorFactory::new(self.provider.tx_ref());
        let trie_cursor_factory =
            InMemoryTrieCursorFactory::new(db_trie_cursor_factory, self.trie_updates.as_ref());
        trie_cursor_factory.account_trie_cursor()
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let db_trie_cursor_factory = DatabaseTrieCursorFactory::new(self.provider.tx_ref());
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
