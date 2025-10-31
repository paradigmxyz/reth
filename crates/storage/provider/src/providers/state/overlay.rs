use alloy_primitives::{BlockNumber, B256};
use reth_db_api::DatabaseError;
use reth_errors::{ProviderError, ProviderResult};
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
use std::sync::Arc;
use tracing::debug;

#[cfg(feature = "metrics")]
#[path = "overlay_metrics.rs"]
mod overlay_metrics;
#[cfg(feature = "metrics")]
use overlay_metrics::{OverlayMetricsTimer, OverlayStateProviderMetrics};

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
    /// Metrics for overlay state provider operations
    #[cfg(feature = "metrics")]
    metrics: OverlayStateProviderMetrics,
}

impl<F> OverlayStateProviderFactory<F> {
    /// Create a new overlay state provider factory
    #[cfg(not(feature = "metrics"))]
    pub const fn new(factory: F) -> Self {
        Self { factory, block_hash: None, trie_overlay: None, hashed_state_overlay: None }
    }

    /// Create a new overlay state provider factory
    #[cfg(feature = "metrics")]
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
    pub fn with_block_hash(mut self, block_hash: Option<B256>) -> Self {
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
            #[cfg(feature = "metrics")]
            let _lookup_timer =
                OverlayMetricsTimer::start(&self.metrics.block_hash_lookup_duration);
            let block_number = provider
                .convert_hash_or_number(block_hash.into())?
                .ok_or_else(|| ProviderError::BlockHashNotFound(block_hash))?;
            Ok(Some(block_number))
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
        #[cfg(feature = "metrics")]
        let _validation_timer =
            OverlayMetricsTimer::start(&self.metrics.reverts_validation_duration);

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
            #[cfg(feature = "metrics")]
            self.metrics.reverts_not_required.increment(1);

            return Ok(false)
        }

        // Extract the lower bound from prune checkpoint if available
        // The prune checkpoint's block_number is the highest pruned block, so data is available
        // starting from the next block
        let lower_bound = prune_checkpoint
            .and_then(|chk| chk.block_number)
            .map(|block_number| block_number + 1)
            .ok_or_else(|| ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: 0..=upper_bound,
            })?;

        let available_range = lower_bound..=upper_bound;

        // Check if the requested block is within the available range
        if !available_range.contains(&requested_block) {
            return Err(ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: available_range,
            });
        }

        let checkpoint_delta = upper_bound - requested_block;

        #[cfg(feature = "metrics")]
        self.metrics.checkpoint_delta.record(checkpoint_delta as f64);

        tracing::info!(
            target: "providers::state::overlay",
            requested_block,
            upper_bound,
            delta = checkpoint_delta,
            "reverts_required: requested != checkpoint, REVERTS NEEDED"
        );

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
    fn database_provider_ro(&self) -> ProviderResult<OverlayStateProvider<F::Provider>> {
        #[cfg(feature = "metrics")]
        let _total_timer =
            OverlayMetricsTimer::start(&self.metrics.total_database_provider_ro_duration);

        let provider = {
            #[cfg(feature = "metrics")]
            let _provider_timer =
                OverlayMetricsTimer::start(&self.metrics.provider_creation_duration);
            self.factory.database_provider_ro()?
        };

        // If block_hash is provided, collect reverts
        let (trie_updates, hashed_state) = if let Some(from_block) =
            self.get_block_number(&provider)? &&
            self.reverts_required(&provider, from_block)?
        {
            #[cfg(feature = "metrics")]
            self.metrics.reverts_required.increment(1);

            // Collect trie reverts
            let mut trie_reverts = {
                #[cfg(feature = "metrics")]
                let _trie_timer = OverlayMetricsTimer::start(&self.metrics.trie_reverts_duration);
                provider.trie_reverts(from_block + 1)?
            };

            #[cfg(feature = "metrics")]
            if trie_reverts.is_empty() {
                self.metrics.trie_reverts_empty.increment(1);
            } else {
                self.metrics.trie_reverts_nonempty.increment(1);
            }

            // Collect state reverts
            //
            // TODO(mediocregopher) make from_reverts return sorted
            // https://github.com/paradigmxyz/reth/issues/19382
            let mut hashed_state_reverts = {
                #[cfg(feature = "metrics")]
                let _from_reverts_timer =
                    OverlayMetricsTimer::start(&self.metrics.from_reverts_duration);
                HashedPostState::from_reverts::<KeccakKeyHasher>(
                    provider.tx_ref(),
                    from_block + 1..,
                )?
                .into_sorted()
            };

            #[cfg(feature = "metrics")]
            if hashed_state_reverts.is_empty() {
                self.metrics.state_reverts_empty.increment(1);
            } else {
                self.metrics.state_reverts_nonempty.increment(1);
            }

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

            debug!(
                target: "providers::state::overlay",
                block_hash = ?self.block_hash,
                ?from_block,
                num_trie_updates = ?trie_updates.total_len(),
                num_state_updates = ?hashed_state_updates.total_len(),
                "Reverted to target block",
            );

            (trie_updates, hashed_state_updates)
        } else {
            #[cfg(feature = "metrics")]
            self.metrics.block_hash_not_set.increment(1);

            // If no block_hash, use overlays directly or defaults
            let trie_updates =
                self.trie_overlay.clone().unwrap_or_else(|| Arc::new(TrieUpdatesSorted::default()));
            let hashed_state = self
                .hashed_state_overlay
                .clone()
                .unwrap_or_else(|| Arc::new(HashedPostStateSorted::default()));

            (trie_updates, hashed_state)
        };

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
