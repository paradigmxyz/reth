use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber, B256};
use metrics::{Counter, Histogram};
use reth_chain_state::{EthPrimitives, StateTrieOverlayManager};
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
    /// Number of managed overlay creations skipped because the reused sparse trie already covers
    /// the DB tip to parent range.
    sparse_trie_overlay_skips: Counter,
}

/// Contains all fields required to initialize an [`OverlayStateProvider`].
#[derive(Debug, Clone)]
pub(super) struct Overlay {
    pub(super) trie_updates: Arc<TrieUpdatesSorted>,
    pub(super) hashed_post_state: Arc<HashedPostStateSorted>,
}

impl Overlay {
    fn empty() -> Self {
        Self {
            trie_updates: Arc::new(TrieUpdatesSorted::default()),
            hashed_post_state: Arc::new(HashedPostStateSorted::default()),
        }
    }
}

#[derive(Debug)]
struct OverlayRevertPlan {
    revert_blocks: Option<RangeInclusive<BlockNumber>>,
    overlay_anchor_hash: BlockHash,
}

/// Returns the highest blocks whose state/trie data and non-state/trie data are durably
/// available in the database.
pub(crate) fn database_state_frontiers<Provider>(
    provider: &Provider,
) -> ProviderResult<(BlockNumHash, BlockNumHash)>
where
    Provider: StageCheckpointReader + BlockNumReader,
{
    let checkpoint = provider
        .get_stage_checkpoint(StageId::Finish)?
        .ok_or_else(|| ProviderError::InsufficientChangesets { requested: 0, available: 0..=0 })?;
    let state_trie_tip_number = checkpoint
        .finish_stage_checkpoint()
        .and_then(|finish| finish.partial_state_trie())
        .unwrap_or(checkpoint.block_number);
    let state_trie_tip_hash = provider
        .convert_number(state_trie_tip_number.into())?
        .ok_or_else(|| ProviderError::HeaderNotFound(state_trie_tip_number.into()))?;
    let finish_tip_number = checkpoint.block_number;
    let finish_tip_hash = provider
        .convert_number(finish_tip_number.into())?
        .ok_or_else(|| ProviderError::HeaderNotFound(finish_tip_number.into()))?;

    Ok((
        BlockNumHash::new(state_trie_tip_number, state_trie_tip_hash),
        BlockNumHash::new(finish_tip_number, finish_tip_hash),
    ))
}

/// Source of overlay data for [`OverlayStateProviderFactory`].
#[derive(Debug, Clone)]
pub(super) enum OverlaySource<N: NodePrimitives = EthPrimitives> {
    /// Immediate overlay with already-computed data.
    Immediate {
        /// Trie updates overlay.
        ///
        /// This can be non-empty when a caller starts with an explicit `TrieInputSorted`, such
        /// as historical providers.
        trie: Arc<TrieUpdatesSorted>,
        /// Hashed state overlay.
        state: Arc<HashedPostStateSorted>,
    },
    /// Manager-backed overlay for in-memory state.
    Managed {
        /// Manager used to resolve in-memory parent state if the parent is not persisted.
        manager: StateTrieOverlayManager<N>,
    },
}

/// Builder for calculating trie and hashed-state overlays.
///
/// This stores the overlay configuration and the logic for resolving overlays and collecting
/// reverts. It is intentionally independent from any provider factory or overlay cache.
#[derive(Debug, Clone)]
pub struct OverlayBuilder<N: NodePrimitives = EthPrimitives> {
    /// Parent hash requested by the caller.
    parent_hash: B256,
    /// Optional overlay source.
    overlay_source: Option<OverlaySource<N>>,
    /// Changeset cache handle for retrieving trie changesets
    changeset_cache: ChangesetCache,
    /// Anchor hash of the reused sparse trie, if this task reused one.
    reused_sparse_trie_anchor_hash: Option<B256>,
    /// Metrics for tracking provider operations
    metrics: OverlayStateProviderMetrics,
}

impl<N: NodePrimitives> OverlayBuilder<N> {
    /// Create a new overlay builder.
    pub fn new(parent_hash: B256, changeset_cache: ChangesetCache) -> Self {
        Self {
            parent_hash,
            overlay_source: None,
            changeset_cache,
            reused_sparse_trie_anchor_hash: None,
            metrics: OverlayStateProviderMetrics::default(),
        }
    }

    /// Set the overlay source.
    ///
    /// This overlay will be applied on top of any reverts.
    pub(super) fn with_overlay_source(mut self, source: Option<OverlaySource<N>>) -> Self {
        self.overlay_source = source;
        self
    }

    /// Skips managed overlay construction when the sparse trie was reused and the DB tip is
    /// already covered by its anchor-to-parent range.
    pub const fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.reused_sparse_trie_anchor_hash = Some(anchor_hash);
        self
    }

    /// Set the state trie overlay manager used to resolve in-memory parent state.
    pub fn with_state_trie_overlay_manager(
        mut self,
        state_trie_overlay_manager: StateTrieOverlayManager<N>,
    ) -> Self {
        self.overlay_source = Some(OverlaySource::Managed { manager: state_trie_overlay_manager });
        self
    }

    /// Set the hashed state overlay.
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        if let Some(new_state) = hashed_state_overlay {
            match &mut self.overlay_source {
                Some(OverlaySource::Immediate { state, .. }) => *state = new_state,
                Some(OverlaySource::Managed { .. }) | None => {
                    self.overlay_source = Some(OverlaySource::Immediate {
                        trie: Arc::new(TrieUpdatesSorted::default()),
                        state: new_state,
                    });
                }
            }
        }
        self
    }

    /// Set the trie updates overlay.
    ///
    /// Only applies to an immediate overlay: a managed overlay's trie updates are resolved from
    /// its manager instead.
    pub fn with_trie_updates_overlay(
        mut self,
        trie_updates_overlay: Option<Arc<TrieUpdatesSorted>>,
    ) -> Self {
        if let Some(trie) = trie_updates_overlay {
            match &mut self.overlay_source {
                Some(OverlaySource::Immediate { trie: existing, .. }) => *existing = trie,
                Some(OverlaySource::Managed { .. }) => {}
                None => {
                    self.overlay_source = Some(OverlaySource::Immediate {
                        trie,
                        state: Arc::new(HashedPostStateSorted::default()),
                    });
                }
            }
        }
        self
    }

    /// Resolves the effective overlay (trie updates, hashed state).
    fn resolve_overlays(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>)> {
        match &self.overlay_source {
            Some(OverlaySource::Managed { manager }) => {
                if anchor_hash == self.parent_hash {
                    Ok((
                        Arc::new(TrieUpdatesSorted::default()),
                        Arc::new(HashedPostStateSorted::default()),
                    ))
                } else {
                    manager
                        .overlay_for_parent(self.parent_hash, anchor_hash)
                        .map_err(ProviderError::other)
                }
            }
            Some(OverlaySource::Immediate { trie, state }) => {
                if anchor_hash != self.parent_hash {
                    return Err(ProviderError::other(std::io::Error::other(format!(
                        "anchor_hash {anchor_hash} doesn't match OverlayBuilder's configured parent ({})",
                        self.parent_hash
                    ))))
                }
                Ok((Arc::clone(trie), Arc::clone(state)))
            }
            None => Ok((
                Arc::new(TrieUpdatesSorted::default()),
                Arc::new(HashedPostStateSorted::default()),
            )),
        }
    }

    /// Returns true if managed overlay resolution can be skipped for this builder.
    fn should_skip_overlay_for_reused_sparse_trie(
        &self,
        state_trie_tip_hash: B256,
        finish_tip_hash: B256,
    ) -> bool {
        let Some(anchor_hash) = self.reused_sparse_trie_anchor_hash else { return false };

        match &self.overlay_source {
            Some(OverlaySource::Managed { manager }) => {
                manager.contains_hash(self.parent_hash, anchor_hash, state_trie_tip_hash) &&
                    manager.contains_hash(self.parent_hash, anchor_hash, finish_tip_hash)
            }
            _ => false,
        }
    }

    /// Returns the revert plan required to expose the requested overlay base state, and validates
    /// that there are sufficient changesets to revert to the requested block number if so.
    ///
    /// Takes into account both the stage checkpoint and the prune checkpoint to determine the
    /// available data range.
    fn revert_plan<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<OverlayRevertPlan>
    where
        Provider: BlockNumReader + PruneCheckpointReader,
    {
        let overlay_anchor_hash = match &self.overlay_source {
            Some(OverlaySource::Managed { manager }) => manager
                .anchor_for_parent(self.parent_hash, state_trie_tip_block.hash)
                .unwrap_or(self.parent_hash),
            _ => self.parent_hash,
        };
        let overlay_anchor_number = provider
            .convert_hash_or_number(overlay_anchor_hash.into())?
            .ok_or(ProviderError::BlockHashNotFound(overlay_anchor_hash))?;
        let canonical_anchor_hash = provider
            .convert_number(overlay_anchor_number.into())?
            .ok_or_else(|| ProviderError::HeaderNotFound(overlay_anchor_number.into()))?;
        if canonical_anchor_hash != overlay_anchor_hash {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "overlay anchor {overlay_anchor_hash} is not on the durable finish chain at block {overlay_anchor_number} (found {canonical_anchor_hash})",
            ))))
        }

        // With no partial-persistence gap, a parent at the Finish tip is already exposed by the
        // database without an overlay or reverts.
        if state_trie_tip_block.hash == finish_tip_block.hash &&
            finish_tip_block.hash == overlay_anchor_hash
        {
            return Ok(OverlayRevertPlan { revert_blocks: None, overlay_anchor_hash })
        }

        // The database is a hybrid view while the state/trie and Finish frontiers differ. A
        // manager overlay can use that view directly only when its anchor-to-parent path covers
        // both frontiers; otherwise the database must first be reverted to the overlay anchor.
        if let Some(OverlaySource::Managed { manager }) = &self.overlay_source &&
            manager.contains_hash(
                self.parent_hash,
                overlay_anchor_hash,
                state_trie_tip_block.hash,
            ) &&
            manager.contains_hash(self.parent_hash, overlay_anchor_hash, finish_tip_block.hash)
        {
            return Ok(OverlayRevertPlan { revert_blocks: None, overlay_anchor_hash })
        }

        if overlay_anchor_number > state_trie_tip_block.number {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "overlay anchor #{} ({}) is after partial state trie frontier #{} ({}); missing trie updates for blocks #{}..=#{}",
                overlay_anchor_number,
                overlay_anchor_hash,
                state_trie_tip_block.number,
                state_trie_tip_block.hash,
                state_trie_tip_block.number + 1,
                overlay_anchor_number,
            ))))
        }

        // Check account history prune checkpoint to determine the lower bound of available data.
        // The prune checkpoint's block_number is the highest pruned block, so data is available
        // starting from the next block.
        let prune_checkpoint = provider.get_prune_checkpoint(PruneSegment::AccountHistory)?;
        let lower_bound = prune_checkpoint
            .and_then(|chk| chk.block_number)
            .map(|block_number| block_number + 1)
            .unwrap_or_default();
        let available_range = lower_bound..=finish_tip_block.number;
        if !available_range.contains(&overlay_anchor_number) {
            return Err(ProviderError::InsufficientChangesets {
                requested: overlay_anchor_number,
                available: available_range,
            })
        }

        Ok(OverlayRevertPlan {
            revert_blocks: Some(overlay_anchor_number + 1..=finish_tip_block.number),
            overlay_anchor_hash,
        })
    }

    /// Calculates a new [`Overlay`] given a transaction and the current durable frontiers.
    #[instrument(
        level = "debug",
        target = "providers::state::overlay",
        skip_all,
        fields(?state_trie_tip_block, ?finish_tip_block, parent_hash = ?self.parent_hash)
    )]
    fn calculate_overlay<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
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
        let OverlayRevertPlan { revert_blocks, overlay_anchor_hash } =
            self.revert_plan(provider, state_trie_tip_block, finish_tip_block)?;

        // Collect any reverts which are required to bring the DB view back to the anchor hash.
        let (trie_updates, hashed_post_state) = if let Some(revert_blocks) = revert_blocks {
            debug!(
                target: "providers::state::overlay",
                ?revert_blocks,
                %overlay_anchor_hash,
                "Collecting trie reverts for overlay state provider"
            );

            // Collect trie reverts using changeset cache
            let trie_reverts = {
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

            // Resolve overlays and extend reverts with them.
            // If reverts are empty, use overlays directly to avoid cloning.
            let (overlay_trie, overlay_state) = self.resolve_overlays(overlay_anchor_hash)?;

            let trie_updates = if trie_reverts.is_empty() {
                overlay_trie
            } else if !overlay_trie.is_empty() {
                let mut trie_reverts = (*trie_reverts).clone();
                trie_reverts.extend_ref_and_sort(&overlay_trie);
                Arc::new(trie_reverts)
            } else {
                trie_reverts
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
                %overlay_anchor_hash,
                "Reverted to anchor block",
            );

            (trie_updates, hashed_state_updates)
        } else {
            // If no reverts are needed, use the manager overlay directly unless the reused sparse
            // trie already covers both durable frontiers through the requested parent.
            if self.should_skip_overlay_for_reused_sparse_trie(
                state_trie_tip_block.hash,
                finish_tip_block.hash,
            ) {
                debug!(
                    target: "providers::state::overlay",
                    parent_hash = %self.parent_hash,
                    state_trie_tip_hash = %state_trie_tip_block.hash,
                    finish_tip_hash = %finish_tip_block.hash,
                    sparse_trie_anchor_hash = ?self.reused_sparse_trie_anchor_hash,
                    "Skipping overlay construction because reused sparse trie covers durable frontiers to parent"
                );

                self.metrics.sparse_trie_overlay_skips.increment(1);

                return Ok(Overlay::empty())
            }

            let (trie_updates, hashed_post_state) = self.resolve_overlays(overlay_anchor_hash)?;

            retrieve_trie_reverts_duration = Duration::ZERO;
            retrieve_hashed_state_reverts_duration = Duration::ZERO;
            trie_updates_total_len = trie_updates.total_len();
            hashed_state_updates_total_len = hashed_post_state.total_len();

            debug!(
                target: "providers::state::overlay",
                num_trie_updates = trie_updates_total_len,
                num_state_updates = hashed_state_updates_total_len,
                %overlay_anchor_hash,
                "Built overlay directly from durable frontier"
            );

            (trie_updates, hashed_post_state)
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
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;
        self.calculate_overlay(provider, state_trie_tip_block, finish_tip_block)
    }
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
    /// A cache which maps `(state_trie_tip, finish_tip) -> Overlay`.
    ///
    /// Under partial persistence the overlay depends on both durable frontiers, so both hashes are
    /// part of the cache key.
    overlay_cache: Arc<DashMap<(BlockHash, BlockHash), Overlay>>,
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

    /// Skips managed overlay construction when this factory is used by a task that reused a sparse
    /// trie covering both durable frontiers through the parent.
    pub fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.overlay_builder =
            self.overlay_builder.with_skip_overlay_for_reused_sparse_trie(anchor_hash);
        self.overlay_cache = Default::default();
        self
    }

    /// Fetches an [`Overlay`] from the cache based on the current durable frontiers. If there is no
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
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;

        let overlay =
            match self.overlay_cache.entry((state_trie_tip_block.hash, finish_tip_block.hash)) {
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
    #[cfg(feature = "partial-persistence")]
    use crate::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    use alloy_primitives::U256;
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_primitives_traits::Account;
    #[cfg(feature = "partial-persistence")]
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint};
    #[cfg(feature = "partial-persistence")]
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{BranchNodeCompact, ComputedTrieData, HashedPostState, HashedStorage, Nibbles};

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
                HashedStorage::from_iter(false, [(hashed_slot, U256::from(id))]),
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

    #[cfg(feature = "partial-persistence")]
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

    #[cfg(feature = "partial-persistence")]
    fn account_keys(overlay: &Overlay) -> Vec<B256> {
        overlay.hashed_post_state.accounts.iter().map(|(key, _)| *key).collect()
    }

    #[cfg(feature = "partial-persistence")]
    fn account_node_paths(overlay: &Overlay) -> Vec<Nibbles> {
        overlay.trie_updates.account_nodes_ref().iter().map(|(path, _)| *path).collect()
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn managed_overlay_starts_at_state_trie_frontier() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = StateTrieOverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let provider = factory.provider().unwrap();

        for (parent_index, expected_ids) in [(3, vec![3, 4]), (4, vec![3, 4, 5])] {
            let overlay = OverlayBuilder::<EthPrimitives>::new(
                blocks[parent_index].recovered_block().hash(),
                ChangesetCache::new(),
            )
            .with_state_trie_overlay_manager(manager.clone())
            .build_overlay(&provider)
            .unwrap();

            assert_eq!(
                account_keys(&overlay),
                expected_ids.iter().copied().map(B256::with_last_byte).collect::<Vec<_>>()
            );
            assert_eq!(
                account_node_paths(&overlay),
                expected_ids
                    .iter()
                    .copied()
                    .map(|id| Nibbles::from_nibbles([id]))
                    .collect::<Vec<_>>()
            );
        }
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn parent_inside_finish_gap_reverts_to_state_trie_frontier() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = StateTrieOverlayManager::default();
        manager.insert_block(blocks[2].clone());
        let provider = factory.provider().unwrap();
        let builder = OverlayBuilder::<EthPrimitives>::new(
            blocks[2].recovered_block().hash(),
            ChangesetCache::new(),
        )
        .with_state_trie_overlay_manager(manager);
        let (state_trie_tip, finish_tip) = database_state_frontiers(&provider).unwrap();
        let plan = builder.revert_plan(&provider, state_trie_tip, finish_tip).unwrap();

        assert_eq!(plan.overlay_anchor_hash, blocks[1].recovered_block().hash());
        assert_eq!(plan.revert_blocks, Some(2..=3));
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn overlay_cache_is_keyed_by_both_durable_frontiers() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = StateTrieOverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let overlay_factory = OverlayStateProviderFactory::new(
            factory.clone(),
            OverlayBuilder::<EthPrimitives>::new(
                blocks[3].recovered_block().hash(),
                ChangesetCache::new(),
            )
            .with_state_trie_overlay_manager(manager),
        );

        let provider = factory.provider().unwrap();
        let first = overlay_factory.get_overlay(&provider).unwrap();
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
        let second = overlay_factory.get_overlay(&provider).unwrap();
        assert_eq!(account_keys(&second), vec![B256::with_last_byte(4)]);
        assert_eq!(account_node_paths(&second), vec![Nibbles::from_nibbles([4])]);
        assert_eq!(overlay_factory.overlay_cache.len(), 2);
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn overlay_after_state_trie_frontier_requires_managed_coverage() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let error = OverlayBuilder::<EthPrimitives>::new(
            blocks[3].recovered_block().hash(),
            ChangesetCache::new(),
        )
        .build_overlay(&provider)
        .unwrap_err();

        assert!(
            error.to_string().contains("is after partial state trie frontier"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn managed_overlay_skips_manager_for_persisted_parent() {
        let parent_hash = B256::with_last_byte(1);
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default());

        let (trie, state) = builder.resolve_overlays(parent_hash).unwrap();
        assert!(trie.is_empty());
        assert!(state.is_empty());
    }

    #[test]
    fn managed_overlay_errors_if_parent_is_not_persisted_or_managed() {
        let parent_hash = B256::with_last_byte(1);
        let anchor_hash = B256::with_last_byte(2);
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default());

        let err = builder.resolve_overlays(anchor_hash).unwrap_err();

        assert!(err.to_string().contains("cannot be anchored"));
    }

    #[test]
    fn managed_overlay_skip_requires_both_frontiers() {
        let parent_hash = B256::with_last_byte(1);
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default());
        assert!(!builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));

        let builder = builder.with_skip_overlay_for_reused_sparse_trie(parent_hash);
        assert!(builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));
        assert!(!builder
            .should_skip_overlay_for_reused_sparse_trie(B256::with_last_byte(3), parent_hash,));

        let blocks = test_blocks();
        let manager = StateTrieOverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let builder = OverlayBuilder::<EthPrimitives>::new(
            blocks[4].recovered_block().hash(),
            ChangesetCache::default(),
        )
        .with_state_trie_overlay_manager(manager)
        .with_skip_overlay_for_reused_sparse_trie(blocks[1].recovered_block().hash());
        assert!(builder.should_skip_overlay_for_reused_sparse_trie(
            blocks[1].recovered_block().hash(),
            blocks[3].recovered_block().hash(),
        ));

        let builder =
            builder.with_skip_overlay_for_reused_sparse_trie(blocks[2].recovered_block().hash());
        assert!(!builder.should_skip_overlay_for_reused_sparse_trie(
            blocks[1].recovered_block().hash(),
            blocks[3].recovered_block().hash(),
        ));
    }
}
