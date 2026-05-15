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
}

/// Contains all fields required to initialize an [`OverlayStateProvider`].
#[derive(Debug, Clone)]
pub(super) struct Overlay {
    pub(super) trie_updates: Arc<TrieUpdatesSorted>,
    pub(super) hashed_post_state: Arc<HashedPostStateSorted>,
}

#[derive(Debug)]
struct OverlayRevertPlan {
    revert_blocks: Option<RangeInclusive<BlockNumber>>,
    overlay_anchor_hash: BlockHash,
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
    /// Manager-backed overlay for in-memory state, with optional immediate overlay data.
    Managed {
        /// Manager used to resolve in-memory parent state if the parent is not persisted.
        manager: StateTrieOverlayManager<N>,
        /// Immediate hashed state overlay applied on top of any manager-produced overlay.
        ///
        /// This is populated by the `with_hashed_state_overlay` methods.
        state: Arc<HashedPostStateSorted>,
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
            metrics: OverlayStateProviderMetrics::default(),
        }
    }

    /// Set the overlay source.
    ///
    /// This overlay will be applied on top of any reverts.
    pub(super) fn with_overlay_source(mut self, source: Option<OverlaySource<N>>) -> Self {
        debug!(
            target: "providers::state::overlay",
            parent_hash = ?self.parent_hash,
            source = overlay_source_kind(source.as_ref()),
            source_anchor = ?source.as_ref().and_then(overlay_source_anchor),
            source_blocks = ?source.as_ref().and_then(overlay_source_num_blocks),
            "Configuring overlay source"
        );
        self.overlay_source = source;
        self
    }

    /// Set the state trie overlay manager used to resolve in-memory parent state.
    pub fn with_state_trie_overlay_manager(
        mut self,
        state_trie_overlay_manager: StateTrieOverlayManager<N>,
    ) -> Self {
        self.overlay_source = Some(OverlaySource::Managed {
            manager: state_trie_overlay_manager,
            state: Arc::new(HashedPostStateSorted::default()),
        });
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
    /// If no overlay exists, creates an immediate overlay with the given state.
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
    ) -> ProviderResult<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>)> {
        match &self.overlay_source {
            Some(OverlaySource::Managed { manager, state }) => {
                let (trie, mut overlay_state) = if anchor_hash == self.parent_hash {
                    (
                        Arc::new(TrieUpdatesSorted::default()),
                        Arc::new(HashedPostStateSorted::default()),
                    )
                } else {
                    manager
                        .overlay_for_parent(self.parent_hash, anchor_hash)
                        .map_err(ProviderError::other)?
                };

                if overlay_state.is_empty() {
                    overlay_state = Arc::clone(state);
                } else if !state.is_empty() {
                    Arc::make_mut(&mut overlay_state).extend_ref_and_sort(state);
                }

                Ok((trie, overlay_state))
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

    /// Returns the highest blocks whose state/trie data and non-state/trie data are durably
    /// available in the database.
    fn get_db_tip_blocks<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<(BlockNumHash, BlockNumHash)>
    where
        Provider: StageCheckpointReader + BlockNumReader,
    {
        let checkpoint = provider.get_stage_checkpoint(StageId::Finish)?.ok_or_else(|| {
            ProviderError::InsufficientChangesets { requested: 0, available: 0..=0 }
        })?;
        let block_number = checkpoint
            .finish_stage_checkpoint()
            .and_then(|finish| finish.partial_state_trie)
            .unwrap_or(checkpoint.block_number);
        let state_trie_tip_hash = provider
            .convert_number(block_number.into())?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        let finish_tip_number = checkpoint.block_number;
        let finish_tip_hash = provider
            .convert_number(finish_tip_number.into())?
            .ok_or_else(|| ProviderError::HeaderNotFound(finish_tip_number.into()))?;
        debug!(
            target: "providers::state::overlay",
            state_trie_tip_number = block_number,
            state_trie_tip_hash = ?state_trie_tip_hash,
            finish_tip_number,
            finish_tip_hash = ?finish_tip_hash,
            parent_hash = ?self.parent_hash,
            "Loaded database overlay frontiers"
        );
        Ok((
            BlockNumHash::new(block_number, state_trie_tip_hash),
            BlockNumHash::new(finish_tip_number, finish_tip_hash),
        ))
    }

    /// Returns the DB anchor to use before applying any in-memory manager overlay.
    fn overlay_anchor_hash<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
    ) -> ProviderResult<BlockHash>
    where
        Provider: BlockNumReader,
    {
        let Some(OverlaySource::Managed { manager, .. }) = &self.overlay_source else {
            return Ok(self.parent_hash)
        };

        let parent_is_state_trie_persisted = provider
            .convert_hash_or_number(self.parent_hash.into())?
            .is_some_and(|parent_number| parent_number <= state_trie_tip_block.number);
        let anchor_hash = if parent_is_state_trie_persisted {
            self.parent_hash
        } else {
            manager
                .anchor_for_parent(self.parent_hash, state_trie_tip_block.hash)
                .ok_or(ProviderError::BlockHashNotFound(self.parent_hash))?
        };

        debug!(
            target: "providers::state::overlay",
            parent_hash = ?self.parent_hash,
            ?state_trie_tip_block,
            ?anchor_hash,
            parent_is_state_trie_persisted,
            "Resolved overlay anchor"
        );

        Ok(anchor_hash)
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
        anchor_hash: BlockHash,
    ) -> ProviderResult<OverlayRevertPlan>
    where
        Provider: BlockNumReader + PruneCheckpointReader,
    {
        let anchor_number = provider
            .convert_hash_or_number(anchor_hash.into())?
            .ok_or(ProviderError::BlockHashNotFound(anchor_hash))?;
        let anchor_hash_at_number = provider
            .convert_number(anchor_number.into())?
            .ok_or_else(|| ProviderError::HeaderNotFound(anchor_number.into()))?;
        if anchor_hash_at_number != anchor_hash {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "anchor hash {} is not on the durable finish chain at block {} (found {})",
                anchor_hash, anchor_number, anchor_hash_at_number,
            ))));
        }

        // If the selected anchor is the current durable state-trie frontier, the database already
        // exposes the right base trie. Any masked blocks above it come from the in-memory overlay.
        if anchor_hash == state_trie_tip_block.hash {
            debug!(
                target: "providers::state::overlay",
                parent_hash = ?self.parent_hash,
                ?anchor_hash,
                ?state_trie_tip_block,
                ?finish_tip_block,
                overlay_anchor_hash = ?state_trie_tip_block.hash,
                "Overlay anchor matches durable state-trie frontier; no reverts required"
            );
            return Ok(OverlayRevertPlan {
                revert_blocks: None,
                overlay_anchor_hash: state_trie_tip_block.hash,
            });
        }

        if anchor_number > state_trie_tip_block.number {
            return Err(ProviderError::other(std::io::Error::other(format!(
                "overlay anchor #{} ({}) is after partial state trie frontier #{} ({}); missing trie updates for blocks #{}..=#{}",
                anchor_number,
                anchor_hash,
                state_trie_tip_block.number,
                state_trie_tip_block.hash,
                state_trie_tip_block.number + 1,
                anchor_number,
            ))));
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

        debug!(
            target: "providers::state::overlay",
            parent_hash = ?self.parent_hash,
            ?anchor_hash,
            anchor_number,
            ?state_trie_tip_block,
            ?finish_tip_block,
            prune_lower_bound = lower_bound,
            available_start = *available_range.start(),
            available_end = *available_range.end(),
            "Checking overlay revert requirements"
        );

        // Check if the requested block is within the available range
        if !available_range.contains(&anchor_number) {
            return Err(ProviderError::InsufficientChangesets {
                requested: anchor_number,
                available: available_range,
            });
        }

        let revert_range = anchor_number + 1..=finish_tip_block.number;
        debug!(
            target: "providers::state::overlay",
            parent_hash = ?self.parent_hash,
            ?anchor_hash,
            anchor_number,
            revert_start = *revert_range.start(),
            revert_end = *revert_range.end(),
            overlay_anchor_hash = ?anchor_hash,
            "Overlay reverts required"
        );

        Ok(OverlayRevertPlan {
            revert_blocks: Some(revert_range),
            overlay_anchor_hash: anchor_hash,
        })
    }

    /// Calculates a new [`Overlay`] given a transaction and the current durable state/trie
    /// frontier.
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
        let anchor_hash = self.overlay_anchor_hash(provider, state_trie_tip_block)?;

        let OverlayRevertPlan { revert_blocks, overlay_anchor_hash } =
            self.revert_plan(provider, state_trie_tip_block, finish_tip_block, anchor_hash)?;

        // Collect any reverts which are required to bring the DB view back to the overlay anchor
        // hash.
        let (trie_updates, hashed_post_state) = if let Some(revert_blocks) = revert_blocks {
            debug!(
                target: "providers::state::overlay",
                ?revert_blocks,
                overlay_anchor_hash = ?overlay_anchor_hash,
                source = overlay_source_kind(self.overlay_source.as_ref()),
                source_anchor = ?self.overlay_source.as_ref().and_then(overlay_source_anchor),
                source_blocks = ?self.overlay_source.as_ref().and_then(overlay_source_num_blocks),
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

            // Resolve overlays and extend reverts with them.
            // If reverts are empty, use overlays directly to avoid cloning.
            let (overlay_trie, overlay_state) = self.resolve_overlays(overlay_anchor_hash)?;

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
                overlay_anchor_hash = ?overlay_anchor_hash,
                source = overlay_source_kind(self.overlay_source.as_ref()),
                "Built overlay after reverting to anchor",
            );

            (trie_updates, hashed_state_updates)
        } else {
            // If no reverts are needed then the overlay can be resolved directly from the durable
            // logical frontier selected by the revert plan.
            let (trie_updates, hashed_state) = self.resolve_overlays(overlay_anchor_hash)?;

            retrieve_trie_reverts_duration = Duration::ZERO;
            retrieve_hashed_state_reverts_duration = Duration::ZERO;
            trie_updates_total_len = trie_updates.total_len();
            hashed_state_updates_total_len = hashed_state.total_len();

            debug!(
                target: "providers::state::overlay",
                num_trie_updates = trie_updates_total_len,
                num_state_updates = hashed_state_updates_total_len,
                overlay_anchor_hash = ?overlay_anchor_hash,
                source = overlay_source_kind(self.overlay_source.as_ref()),
                "Built overlay directly from durable frontier"
            );

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
        let (state_trie_tip_block, finish_tip_block) = self.get_db_tip_blocks(provider)?;
        debug!(
            target: "providers::state::overlay",
            parent_hash = ?self.parent_hash,
            ?state_trie_tip_block,
            ?finish_tip_block,
            source = overlay_source_kind(self.overlay_source.as_ref()),
            source_anchor = ?self.overlay_source.as_ref().and_then(overlay_source_anchor),
            source_blocks = ?self.overlay_source.as_ref().and_then(overlay_source_num_blocks),
            "Building overlay"
        );
        self.calculate_overlay(provider, state_trie_tip_block, finish_tip_block)
    }
}

fn overlay_source_kind<N: NodePrimitives>(source: Option<&OverlaySource<N>>) -> &'static str {
    match source {
        Some(OverlaySource::Immediate { .. }) => "immediate",
        Some(OverlaySource::Managed { .. }) => "managed",
        None => "none",
    }
}

fn overlay_source_anchor<N: NodePrimitives>(source: &OverlaySource<N>) -> Option<BlockHash> {
    match source {
        OverlaySource::Immediate { .. } => None,
        OverlaySource::Managed { .. } => None,
    }
}

fn overlay_source_num_blocks<N: NodePrimitives>(source: &OverlaySource<N>) -> Option<usize> {
    match source {
        OverlaySource::Immediate { .. } => None,
        OverlaySource::Managed { .. } => None,
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
    /// A cache which maps `(state_trie_tip_hash, finish_tip_hash) -> Overlay`.
    ///
    /// Under partial persistence the overlay depends on both the durable trie frontier and the
    /// fully durable Finish frontier, so both hashes are part of the cache key.
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

    /// Extends the existing hashed state overlay with the given [`HashedPostStateSorted`].
    pub fn with_extended_hashed_state_overlay(mut self, other: HashedPostStateSorted) -> Self {
        self.overlay_builder = self.overlay_builder.with_extended_hashed_state_overlay(other);
        self.overlay_cache = Default::default();
        self
    }

    /// Fetches an [`Overlay`] from the cache based on the current durable frontiers. If there is
    /// no cached value then this calculates the [`Overlay`] and populates the cache.
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
        let (state_trie_tip_block, finish_tip_block) =
            self.overlay_builder.get_db_tip_blocks(provider)?;

        let overlay = match self
            .overlay_cache
            .entry((state_trie_tip_block.hash, finish_tip_block.hash))
        {
            dashmap::Entry::Occupied(entry) => {
                debug!(
                    target: "providers::state::overlay",
                    parent_hash = ?self.overlay_builder.parent_hash,
                    ?state_trie_tip_block,
                    ?finish_tip_block,
                    source = overlay_source_kind(self.overlay_builder.overlay_source.as_ref()),
                    "Using cached overlay"
                );
                entry.get().clone()
            }
            dashmap::Entry::Vacant(entry) => {
                self.overlay_builder.metrics.overlay_cache_misses.increment(1);
                debug!(
                    target: "providers::state::overlay",
                    parent_hash = ?self.overlay_builder.parent_hash,
                    ?state_trie_tip_block,
                    ?finish_tip_block,
                    source = overlay_source_kind(self.overlay_builder.overlay_source.as_ref()),
                    source_anchor = ?self.overlay_builder.overlay_source.as_ref().and_then(overlay_source_anchor),
                    source_blocks = ?self.overlay_builder.overlay_source.as_ref().and_then(overlay_source_num_blocks),
                    "Overlay cache miss"
                );
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
    use crate::{
        test_utils::create_test_provider_factory, BlockWriter, SaveBlocksMode, SaveBlocksPlan,
        SaveBlocksPlanStep,
    };
    use alloy_primitives::{B256, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ComputedTrieData, ExecutedBlock};
    use reth_primitives_traits::Account;
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint};
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{updates::TrieUpdatesSorted, HashedPostState, HashedStorage};
    use std::sync::Arc;

    fn full_save_plan(
        blocks: impl IntoIterator<Item = ExecutedBlock<EthPrimitives>>,
    ) -> SaveBlocksPlan<EthPrimitives> {
        let blocks = blocks.into_iter().collect::<Vec<_>>();
        let full_range = 0..blocks.len();
        SaveBlocksPlan::new(
            blocks,
            vec![SaveBlocksPlanStep::new(
                full_range.clone(),
                Some(full_range.end..full_range.end),
                true,
            )],
        )
    }

    fn partial_save_plan(
        blocks: impl IntoIterator<Item = ExecutedBlock<EthPrimitives>>,
        steps: Vec<SaveBlocksPlanStep>,
    ) -> SaveBlocksPlan<EthPrimitives> {
        SaveBlocksPlan::new(blocks.into_iter().collect(), steps)
    }

    fn with_unique_state(
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

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::clone(&block.execution_output),
            ComputedTrieData::new(Arc::new(hashed_state), Arc::new(TrieUpdatesSorted::default())),
        )
    }

    #[test]
    fn managed_overlay_covers_masked_blocks_above_state_trie_frontier() {
        let factory = create_test_provider_factory();
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder
            .get_executed_blocks(0..5)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect::<Vec<_>>();

        let state_trie_tip = &blocks[1];
        let finish_tip = &blocks[3];
        let manager = StateTrieOverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(blocks[0].recovered_block()).unwrap();
        provider_rw.insert_block(state_trie_tip.recovered_block()).unwrap();
        provider_rw.insert_block(blocks[2].recovered_block()).unwrap();
        provider_rw.insert_block(finish_tip.recovered_block()).unwrap();
        provider_rw
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(finish_tip.block_number()).with_finish_stage_checkpoint(
                    FinishCheckpoint { partial_state_trie: Some(state_trie_tip.block_number()) },
                ),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let overlay = OverlayBuilder::<EthPrimitives>::new(
            blocks[4].recovered_block().hash(),
            ChangesetCache::new(),
        )
        .with_state_trie_overlay_manager(manager)
        .build_overlay(&provider)
        .unwrap();

        assert_eq!(overlay.hashed_post_state.accounts.len(), 3);
    }

    #[test]
    fn build_overlay_errors_for_anchor_after_state_trie_frontier() {
        let factory = create_test_provider_factory();
        let mut block_builder = TestBlockBuilder::eth().with_state();

        let genesis = block_builder.get_executed_blocks(0..1).next().unwrap();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_blocks(
                &full_save_plan(std::slice::from_ref(&genesis).to_vec()),
                SaveBlocksMode::Full,
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_blocks(
                &partial_save_plan(
                    blocks.clone(),
                    vec![
                        SaveBlocksPlanStep::new(0..1, Some(1..3), true),
                        SaveBlocksPlanStep::new(1..3, None, true),
                    ],
                ),
                SaveBlocksMode::Full,
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let anchor = blocks[1].recovered_block().hash();
        let error = OverlayBuilder::<EthPrimitives>::new(anchor, ChangesetCache::new())
            .build_overlay(&provider)
            .unwrap_err();

        assert!(
            error.to_string().contains("is after partial state trie frontier"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn managed_overlay_uses_state_trie_frontier_as_anchor() {
        let factory = create_test_provider_factory();
        let mut block_builder = TestBlockBuilder::eth();
        let blocks = block_builder
            .get_executed_blocks(0..5)
            .enumerate()
            .map(|(index, block)| with_unique_state(&block, index as u8 + 1))
            .collect::<Vec<_>>();

        let state_trie_tip = &blocks[1];
        let finish_tip = &blocks[3];
        let manager = StateTrieOverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw.insert_block(blocks[0].recovered_block()).unwrap();
        provider_rw.insert_block(state_trie_tip.recovered_block()).unwrap();
        provider_rw.insert_block(blocks[2].recovered_block()).unwrap();
        provider_rw.insert_block(finish_tip.recovered_block()).unwrap();
        provider_rw
            .save_stage_checkpoint(
                StageId::Finish,
                StageCheckpoint::new(finish_tip.block_number()).with_finish_stage_checkpoint(
                    FinishCheckpoint { partial_state_trie: Some(state_trie_tip.block_number()) },
                ),
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let overlay = OverlayBuilder::<EthPrimitives>::new(
            blocks[4].recovered_block().hash(),
            ChangesetCache::new(),
        )
        .with_state_trie_overlay_manager(manager)
        .build_overlay(&provider)
        .unwrap();

        assert_eq!(overlay.hashed_post_state.accounts.len(), 3);
    }

    #[test]
    fn build_overlay_errors_for_finish_anchor_after_state_trie_frontier() {
        let factory = create_test_provider_factory();
        let mut block_builder = TestBlockBuilder::eth().with_state();

        let genesis = block_builder.get_executed_blocks(0..1).next().unwrap();
        let blocks = block_builder.get_executed_blocks(1..4).collect::<Vec<_>>();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_blocks(
                &full_save_plan(std::slice::from_ref(&genesis).to_vec()),
                SaveBlocksMode::Full,
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_blocks(
                &partial_save_plan(
                    blocks.clone(),
                    vec![
                        SaveBlocksPlanStep::new(0..1, Some(1..3), true),
                        SaveBlocksPlanStep::new(1..3, None, true),
                    ],
                ),
                SaveBlocksMode::Full,
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let provider = factory.provider().unwrap();
        let finish_anchor = blocks[2].recovered_block().hash();

        let error = OverlayBuilder::<EthPrimitives>::new(finish_anchor, ChangesetCache::new())
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
    fn extending_hashed_state_keeps_managed_overlay_source() {
        let parent_hash = B256::with_last_byte(1);
        let hashed_state = HashedPostState::default()
            .with_accounts([(B256::with_last_byte(2), Some(Account::default()))])
            .into_sorted();
        let builder = OverlayBuilder::<EthPrimitives>::new(parent_hash, ChangesetCache::default())
            .with_state_trie_overlay_manager(StateTrieOverlayManager::default())
            .with_extended_hashed_state_overlay(hashed_state);

        let Some(OverlaySource::Managed { state, .. }) = builder.overlay_source else {
            panic!("expected managed overlay source")
        };
        assert_eq!(state.total_len(), 1);
    }
}
