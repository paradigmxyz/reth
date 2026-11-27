use crate::{
    context::OverlayCacheConfig, ExecutionOverlay, OverlayContext, OverlayError,
    OverlayStateProviderMode, OverlayView,
};
use alloy_eips::BlockNumHash;
use alloy_primitives::{BlockHash, BlockNumber, B256};
use reth_chain_state::{BlockState, ExecutedBlock, InMemoryStateSnapshot};
use reth_errors::{ProviderError, ProviderResult};
use reth_ethereum_primitives::EthPrimitives;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use reth_trie_db::DatabaseHashedPostState;
use std::{
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, debug_span, instrument};

/// Contains the trie and hashed-state data required to initialize an overlay state provider.
#[derive(Debug, Clone)]
pub struct StateTrieOverlay {
    input: TrieInputSorted,
    /// Whether construction was skipped because a reused sparse trie covers this range.
    skipped_for_reused_sparse_trie: bool,
}

impl StateTrieOverlay {
    pub(crate) const fn new(input: TrieInputSorted) -> Self {
        Self { input, skipped_for_reused_sparse_trie: false }
    }

    fn empty() -> Self {
        Self { input: TrieInputSorted::default(), skipped_for_reused_sparse_trie: true }
    }

    /// Returns the trie input represented by this overlay.
    pub const fn input(&self) -> &TrieInputSorted {
        &self.input
    }

    pub(crate) const fn skipped_for_reused_sparse_trie(&self) -> bool {
        self.skipped_for_reused_sparse_trie
    }
}

/// Builds state trie and execution overlays from the in-memory blocks paired with a database view.
/// The associated context supplies the shared caches.
#[derive(Debug, Clone)]
pub(crate) struct OverlayBuilder<N: NodePrimitives = EthPrimitives> {
    /// Block whose post-state is requested.
    parent_hash: B256,
    /// The in-memory chain ending at the requested block, if one is tracked.
    parent_state: Option<Arc<BlockState<N>>>,
    /// Snapshot of the tracked blocks and their associated caches.
    snapshot: OverlaySnapshot<N>,
    /// Anchor hash of the reused sparse trie, if this task reused one.
    reused_sparse_trie_anchor_hash: Option<B256>,
    /// Whether building the overlay may query revert changesets.
    allow_reverts: bool,
    /// Cache behavior for the overlays this builder resolves.
    overlay_cache_config: OverlayCacheConfig,
    /// Cache behavior for trie revert ranges.
    trie_revert_cache: TrieRevertCache,
}

impl<N: NodePrimitives> OverlayBuilder<N> {
    /// Resolves the requested block against the supplied view of the in-memory blocks.
    pub(crate) fn new(
        parent_hash: B256,
        snapshot: OverlaySnapshot<N>,
        mode: OverlayBuildMode<N>,
    ) -> Self {
        let mut builder = Self {
            parent_hash,
            parent_state: snapshot.blocks.state_by_hash(parent_hash),
            snapshot,
            reused_sparse_trie_anchor_hash: None,
            allow_reverts: true,
            overlay_cache_config: OverlayCacheConfig::default(),
            trie_revert_cache: TrieRevertCache::ReadWrite,
        };
        match mode {
            OverlayBuildMode::Provider(OverlayStateProviderMode::Cached) => {}
            OverlayBuildMode::Provider(OverlayStateProviderMode::UncachedReverts) => {
                builder.trie_revert_cache = TrieRevertCache::ReadOnly;
            }
            OverlayBuildMode::Provider(OverlayStateProviderMode::AppendBlock(block)) => {
                assert_eq!(
                    block.recovered_block().parent_hash(),
                    parent_hash,
                    "appended block must extend the requested parent"
                );
                builder.parent_hash = block.recovered_block().hash();
                builder.parent_state =
                    Some(Arc::new(BlockState::with_parent(block, builder.parent_state.take())));
                builder.overlay_cache_config.write_to_cache = false;
            }
            OverlayBuildMode::ReusedSparseTrie { anchor_hash } => {
                builder.reused_sparse_trie_anchor_hash = Some(anchor_hash);
            }
            OverlayBuildMode::Finish => {
                builder.allow_reverts = false;
            }
        }
        builder
    }

    /// Returns the in-memory chain ending at the requested block, if it is tracked.
    pub(crate) const fn parent_state(&self) -> Option<&Arc<BlockState<N>>> {
        self.parent_state.as_ref()
    }

    /// Returns the durable anchor to use for this builder's parent.
    #[cfg(test)]
    fn anchor_at_parent<Provider>(&self, provider: &Provider) -> ProviderResult<AnchorForParent>
    where
        Provider: StageCheckpointReader
            + BlockNumReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader,
    {
        let (partial_state_trie, finish) = database_state_frontiers(provider)?;
        self.anchor_at_parent_with_frontiers(provider, partial_state_trie, finish)
    }

    /// Returns the durable anchor to use for this builder's parent using known frontiers.
    fn anchor_at_parent_with_frontiers<Provider>(
        &self,
        provider: &Provider,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
    ) -> ProviderResult<AnchorForParent>
    where
        Provider: BlockNumReader + PruneCheckpointReader + ChangeSetReader + StorageChangeSetReader,
    {
        let mut in_mem_chain = self
            .parent_state
            .iter()
            .flat_map(|state| state.chain())
            .map(BlockState::block_ref)
            .peekable();
        let persisted_parent = match in_mem_chain
            .peek()
            .filter(|block| block.recovered_block().hash() == self.parent_hash)
            .map(|block| block.recovered_block().number())
        {
            Some(parent_number) if parent_number > partial_state_trie.number => None,
            Some(parent_number)
                if parent_number == partial_state_trie.number &&
                    self.parent_hash == partial_state_trie.hash =>
            {
                Some(parent_number)
            }
            Some(parent_number) => (provider.block_hash(parent_number)? == Some(self.parent_hash))
                .then_some(parent_number),
            None if self.parent_hash == partial_state_trie.hash => Some(partial_state_trie.number),
            None => provider
                .block_number(self.parent_hash)?
                .filter(|&number| number <= partial_state_trie.number),
        };

        let mut finish_seen = self.parent_hash == finish.hash;
        let anchor = if let Some(parent_number) = persisted_parent {
            BlockNumHash::new(parent_number, self.parent_hash)
        } else {
            let mut in_mem_chain = in_mem_chain.inspect(|block| {
                finish_seen |= block.recovered_block().hash() == finish.hash;
            });

            if let Some(anchor) =
                anchor_for_parent_in(self.parent_hash, &mut in_mem_chain, partial_state_trie)
            {
                anchor
            } else {
                let anchor_number = provider
                    .convert_hash_or_number(self.parent_hash.into())?
                    .ok_or(ProviderError::BlockHashNotFound(self.parent_hash))?;
                BlockNumHash::new(anchor_number, self.parent_hash)
            }
        };

        finish_seen |= anchor.hash == finish.hash;

        if anchor.number > partial_state_trie.number {
            return Err(ProviderError::other(OverlayError::AnchorAfterTrieFrontier {
                anchor,
                trie_frontier: partial_state_trie,
            }))
        }

        // If the Finish block (db tip) was seen in the in-memory chain then we know that anchor is
        // on the same chain as partial_state_trie as well. Given that anchor <= partial_state_trie,
        // we can be sure that the in-memory chain is a superset of partial_state_trie+1..finish,
        // and therefore can be used without reverts.
        if finish_seen {
            return Ok(AnchorForParent::NoReverts { anchor })
        }

        // Reverts reconstruct canonical state by block number, so they cannot recover an anchor
        // from another fork even if its number is covered by the available changesets.
        if provider.block_hash(anchor.number)? != Some(anchor.hash) {
            return Err(ProviderError::BlockHashNotFound(anchor.hash))
        }

        // Otherwise reverts are required; we check the changesets to make sure they are actually
        // available before signaling that they are required. The first block of the lowest
        // changeset file bounds them like a prune checkpoint: a node bootstrapped without
        // changesets from genesis has no checkpoint for the history it never had.
        let account_history = provider
            .get_prune_checkpoint(PruneSegment::AccountHistory)?
            .and_then(|checkpoint| checkpoint.block_number)
            .max(provider.lowest_account_changeset_block()?);
        let storage_history = provider
            .get_prune_checkpoint(PruneSegment::StorageHistory)?
            .and_then(|checkpoint| checkpoint.block_number)
            .max(provider.lowest_storage_changeset_block()?);
        let lower_bound = account_history.max(storage_history).unwrap_or_default();
        let available_range = lower_bound..=finish.number;
        if !available_range.contains(&anchor.number) {
            return Err(ProviderError::InsufficientChangesets {
                requested: anchor.number,
                available: available_range,
            })
        }

        Ok(AnchorForParent::RevertsRequired { anchor, finish })
    }

    /// Builds the effective state trie overlay for the given provider.
    ///
    /// See [`Self::build_state_trie_overlay_at_frontiers`] for the `trie_changesets` contract.
    #[cfg(test)]
    #[instrument(level = "debug", target = "storage::overlay", skip_all)]
    fn build_state_trie_overlay<Provider>(
        &self,
        provider: &Provider,
        trie_changesets: bool,
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
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;
        self.build_state_trie_overlay_at_frontiers(
            provider,
            state_trie_tip_block,
            finish_tip_block,
            trie_changesets,
        )
    }

    /// Builds the effective state trie overlay using frontiers already read from the provider.
    ///
    /// This is useful for callers that key an overlay cache by the durable frontiers.
    ///
    /// With `trie_changesets` set, reverted trie nodes are exact, including their hash and tree
    /// masks, as required to produce trie updates. Without it, trie changesets are not read; the
    /// overlay's prefix sets instead invalidate every key changed by reverts or by in-memory blocks
    /// replayed on top of the anchor, so roots and proof hashes are exact but masks may not match
    /// the stored trie.
    #[instrument(
        level = "debug",
        target = "storage::overlay",
        skip_all,
        fields(?state_trie_tip_block, ?finish_tip_block, parent_hash = ?self.parent_hash)
    )]
    pub(crate) fn build_state_trie_overlay_at_frontiers<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
        trie_changesets: bool,
    ) -> ProviderResult<StateTrieOverlay>
    where
        Provider: ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StageCheckpointReader
            + PruneCheckpointReader
            + StorageSettingsCache,
    {
        let retrieve_trie_reverts_duration;
        let retrieve_hashed_state_reverts_duration;
        let trie_updates_total_len;
        let hashed_state_updates_total_len;

        let anchor_for_parent =
            self.anchor_at_parent_with_frontiers(provider, state_trie_tip_block, finish_tip_block)?;

        // Collect any reverts which are required to bring the DB view back to the anchor hash.
        let (trie_updates, hashed_post_state, prefix_sets) = match &anchor_for_parent {
            AnchorForParent::RevertsRequired { anchor, finish } => {
                let revert_blocks = self.revert_range(*anchor, *finish)?;

                debug!(
                    target: "storage::overlay",
                    ?revert_blocks,
                    ?anchor,
                    "Collecting trie reverts for overlay state provider"
                );

                // Trie updates applied directly over the on-disk trie. With trie changesets these
                // are exact reverts down to the anchor's trie; without them they are the in-memory
                // nodes up to Finish (or nothing when the trie is already at Finish), and the
                // prefix sets cover every key that differs from the anchor.
                let base_trie_updates = if trie_changesets {
                    let _guard = debug_span!(target: "storage::overlay", "retrieving_trie_reverts")
                        .entered();
                    let start = Instant::now();
                    let accumulated_reverts =
                        self.snapshot.context.manager().changeset_cache().get_or_compute_range(
                            provider,
                            revert_blocks.clone(),
                            state_trie_tip_block,
                            finish_tip_block,
                            self.trie_revert_cache,
                            &self.snapshot,
                        )?;
                    retrieve_trie_reverts_duration = start.elapsed();
                    accumulated_reverts
                } else if state_trie_tip_block == finish_tip_block {
                    retrieve_trie_reverts_duration = Duration::ZERO;
                    Arc::default()
                } else {
                    // Revert prefixes describe changes since the anchor, not stale hashes in
                    // masked DB rows. Complete the trie at Finish before applying those prefixes.
                    let start = Instant::now();
                    let finish_state =
                        self.snapshot.blocks.state_by_hash(finish_tip_block.hash).ok_or_else(
                            || {
                                ProviderError::other(OverlayError::MissingFinishBlock {
                                    finish: finish_tip_block,
                                    trie_frontier: state_trie_tip_block,
                                })
                            },
                        )?;
                    let (nodes, _) = self
                        .snapshot
                        .context
                        .overlay_for_parent(
                            &finish_state,
                            state_trie_tip_block.hash,
                            OverlayCacheConfig::default(),
                        )
                        .map_err(ProviderError::other)?;
                    retrieve_trie_reverts_duration = start.elapsed();
                    nodes
                };

                let mut hashed_state_reverts = {
                    let _guard =
                        debug_span!(target: "storage::overlay", "retrieving_hashed_state_reverts")
                            .entered();
                    let start = Instant::now();
                    let res = HashedPostStateSorted::from_reverts(provider, revert_blocks)?;
                    retrieve_hashed_state_reverts_duration = start.elapsed();
                    res
                };

                // Resolve overlays and extend reverts with them. If reverts are empty, use overlays
                // directly to avoid cloning.
                let (overlay_trie, overlay_state) =
                    self.resolve_state_trie_overlays(anchor.hash)?;

                // Without trie changesets the base trie updates describe Finish, while the overlay
                // trie nodes are relative to the anchor's trie. Mixing them can leave Finish-only
                // nodes beneath anchor-relative branches, so only the prefix sets may describe the
                // in-memory blocks.
                let trie_updates = if !trie_changesets {
                    base_trie_updates
                } else if base_trie_updates.is_empty() {
                    overlay_trie
                } else if !overlay_trie.is_empty() {
                    let mut base_trie_updates = (*base_trie_updates).clone();
                    base_trie_updates.extend_ref_and_sort(&overlay_trie);
                    Arc::new(base_trie_updates)
                } else {
                    base_trie_updates
                };

                let hashed_state_updates = if hashed_state_reverts.is_empty() {
                    overlay_state
                } else if !overlay_state.is_empty() {
                    hashed_state_reverts.extend_ref_and_sort(&overlay_state);
                    Arc::new(hashed_state_reverts)
                } else {
                    Arc::new(hashed_state_reverts)
                };

                // Without trie changesets the trie nodes still describe Finish, so every key whose
                // value differs from Finish must be invalidated: both the reverted keys and the
                // keys changed by the in-memory blocks replayed on top of the anchor.
                let prefix_sets = if trie_changesets {
                    Default::default()
                } else {
                    hashed_state_updates.construct_prefix_sets()
                };

                trie_updates_total_len = trie_updates.total_len();
                hashed_state_updates_total_len = hashed_state_updates.total_len();

                debug!(
                    target: "storage::overlay",
                    num_trie_updates = ?trie_updates_total_len,
                    num_state_updates = ?hashed_state_updates_total_len,
                    ?anchor,
                    "Reverted to anchor block",
                );

                (trie_updates, hashed_state_updates, prefix_sets)
            }
            AnchorForParent::NoReverts { anchor } => {
                // If no reverts are needed, use the manager overlay directly unless the reused
                // sparse trie already covers both durable frontiers through the
                // requested parent.
                if self.should_skip_overlay_for_reused_sparse_trie(
                    state_trie_tip_block.hash,
                    finish_tip_block.hash,
                ) {
                    self.snapshot
                        .context
                        .manager()
                        .builder_metrics()
                        .sparse_trie_overlay_skips
                        .increment(1);

                    return Ok(StateTrieOverlay::empty())
                }

                let (trie_updates, hashed_post_state) =
                    self.resolve_state_trie_overlays(anchor.hash)?;

                retrieve_trie_reverts_duration = Duration::ZERO;
                retrieve_hashed_state_reverts_duration = Duration::ZERO;
                trie_updates_total_len = trie_updates.total_len();
                hashed_state_updates_total_len = hashed_post_state.total_len();

                debug!(
                    target: "storage::overlay",
                    num_trie_updates = trie_updates_total_len,
                    num_state_updates = hashed_state_updates_total_len,
                    ?anchor,
                    "Built overlay directly from durable frontier"
                );

                (trie_updates, hashed_post_state, Default::default())
            }
        };

        let metrics = self.snapshot.context.manager().builder_metrics();
        metrics.retrieve_trie_reverts_duration.record(retrieve_trie_reverts_duration.as_secs_f64());
        metrics
            .retrieve_hashed_state_reverts_duration
            .record(retrieve_hashed_state_reverts_duration.as_secs_f64());
        metrics.trie_updates_size.record(trie_updates_total_len as f64);
        metrics.hashed_state_size.record(hashed_state_updates_total_len as f64);

        Ok(StateTrieOverlay::new(TrieInputSorted::new(
            trie_updates,
            hashed_post_state,
            prefix_sets,
        )))
    }

    /// Returns the in-memory execution overlay and the block for historical fallback reads.
    #[cfg(test)]
    #[instrument(level = "debug", target = "storage::overlay", skip_all)]
    fn execution_overlay<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<(Arc<ExecutionOverlay>, Option<BlockNumber>)>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader,
    {
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;
        self.execution_overlay_at_frontiers(provider, state_trie_tip_block, finish_tip_block)
    }

    /// Returns the in-memory execution overlay using frontiers already read from the provider.
    #[instrument(
        level = "trace",
        target = "storage::overlay",
        skip_all,
        fields(?state_trie_tip_block, ?finish_tip_block, parent_hash = ?self.parent_hash)
    )]
    pub(crate) fn execution_overlay_at_frontiers<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<(Arc<ExecutionOverlay>, Option<BlockNumber>)>
    where
        Provider: ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + PruneCheckpointReader,
    {
        let anchor_for_parent =
            self.anchor_at_parent_with_frontiers(provider, state_trie_tip_block, finish_tip_block)?;
        let (anchor_hash, fallback_block_number) = match anchor_for_parent {
            AnchorForParent::RevertsRequired { anchor, .. } => {
                (anchor.hash, Some(anchor.number + 1))
            }
            AnchorForParent::NoReverts { anchor } => (anchor.hash, None),
        };
        Ok((self.resolve_execution_overlay(anchor_hash)?, fallback_block_number))
    }

    /// Resolves the effective overlay (trie updates, hashed state).
    fn resolve_state_trie_overlays(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>)> {
        if anchor_hash == self.parent_hash {
            Ok((Arc::new(TrieUpdatesSorted::default()), Arc::new(HashedPostStateSorted::default())))
        } else {
            let parent_state = self.parent_state.as_deref().ok_or_else(|| {
                ProviderError::other(OverlayError::MissingParent(self.parent_hash))
            })?;
            self.snapshot
                .context
                .overlay_for_parent(parent_state, anchor_hash, self.overlay_cache_config)
                .map_err(ProviderError::other)
        }
    }

    /// Resolves the execution overlay from the blocks in the snapshot.
    fn resolve_execution_overlay(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<Arc<ExecutionOverlay>> {
        if anchor_hash == self.parent_hash {
            Ok(Arc::new(ExecutionOverlay::default()))
        } else {
            let parent_state = self.parent_state.as_deref().ok_or_else(|| {
                ProviderError::other(OverlayError::MissingParent(self.parent_hash))
            })?;
            self.snapshot
                .context
                .execution_overlay_for_block_state(
                    parent_state,
                    anchor_hash,
                    self.overlay_cache_config,
                )
                .map_err(ProviderError::other)
        }
    }

    /// Returns the blocks to revert from Finish to the selected anchor.
    fn revert_range(
        &self,
        anchor: BlockNumHash,
        finish: BlockNumHash,
    ) -> ProviderResult<RangeInclusive<BlockNumber>> {
        if !self.allow_reverts {
            return Err(ProviderError::other(OverlayError::UnexpectedReverts {
                parent_hash: self.parent_hash,
                finish,
                anchor,
            }))
        }
        Ok(anchor.number + 1..=finish.number)
    }

    /// Returns true if the reused sparse trie already covers both database frontiers.
    fn should_skip_overlay_for_reused_sparse_trie(
        &self,
        state_trie_tip_hash: B256,
        finish_tip_hash: B256,
    ) -> bool {
        let Some(anchor_hash) = self.reused_sparse_trie_anchor_hash else { return false };

        self.contains_hash(anchor_hash, state_trie_tip_hash) &&
            self.contains_hash(anchor_hash, finish_tip_hash)
    }

    fn contains_hash(&self, anchor_hash: B256, hash: B256) -> bool {
        let mut current_hash = self.parent_hash;
        let mut blocks = self.parent_state.iter().flat_map(|state| state.chain());

        loop {
            if current_hash == hash {
                return true
            }
            if current_hash == anchor_hash {
                return false
            }

            let Some(block) = blocks.next() else { return false };
            current_hash = block.block_ref().recovered_block().parent_hash();
        }
    }
}

/// A block snapshot bound to the context that captured it.
#[derive(Debug, Clone)]
pub(crate) struct OverlaySnapshot<N: NodePrimitives> {
    blocks: InMemoryStateSnapshot<N>,
    context: OverlayContext<N>,
}

impl<N: NodePrimitives> OverlaySnapshot<N> {
    pub(crate) const fn context(&self) -> &OverlayContext<N> {
        &self.context
    }

    pub(crate) fn split<T>(view: OverlayView<T, N>) -> (T, Self) {
        let (value, blocks, context) = view.into_parts();
        (value, Self { blocks, context })
    }

    pub(crate) fn from_view<T>(view: &OverlayView<T, N>) -> Self {
        Self { blocks: view.snapshot().clone(), context: view.context().clone() }
    }
}

pub(crate) enum OverlayBuildMode<N: NodePrimitives> {
    Provider(OverlayStateProviderMode<N>),
    ReusedSparseTrie { anchor_hash: B256 },
    Finish,
}

/// Shared trie revert ranges can be read in either mode.
#[derive(Clone, Copy, Debug)]
pub(crate) enum TrieRevertCache {
    ReadWrite,
    ReadOnly,
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
        .ok_or_else(|| ProviderError::other(OverlayError::MissingFinishCheckpoint))?;
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

fn anchor_for_parent_in<'a, N: NodePrimitives + 'a>(
    parent_hash: B256,
    in_mem_chain: impl Iterator<Item = &'a ExecutedBlock<N>>,
    preferred_anchor: BlockNumHash,
) -> Option<BlockNumHash> {
    if parent_hash == preferred_anchor.hash {
        return Some(preferred_anchor)
    }

    let mut anchor = None;

    for block in in_mem_chain {
        let block_parent = block.recovered_block().parent_num_hash();

        if block_parent.hash == preferred_anchor.hash {
            return Some(preferred_anchor)
        }
        anchor = Some(block_parent);
    }

    anchor
}

/// Describes whether an overlay must revert the database before using its anchor.
#[derive(Debug)]
enum AnchorForParent {
    /// The in-memory chain covers the durable frontiers through this anchor.
    NoReverts {
        /// Block to anchor the overlay to.
        anchor: BlockNumHash,
    },
    /// The database must be reverted from `finish` to `anchor` first.
    RevertsRequired {
        /// Block to anchor the overlay to.
        anchor: BlockNumHash,
        /// Current Finish frontier.
        finish: BlockNumHash,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_utils::{
        account_keys, account_node_paths, assert_same_overlay, overlay_context_with_writer,
        setup_frontiers, test_blocks, with_unique_trie_data,
    };
    use alloy_primitives::{keccak256, Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, NewCanonicalChain};
    use reth_db::{
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_stages_types::StageCheckpoint;
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::Nibbles;

    fn cached_builder<T>(view: &OverlayView<T>, parent_hash: B256) -> OverlayBuilder {
        OverlayBuilder::new(
            parent_hash,
            OverlaySnapshot::from_view(view),
            OverlayBuildMode::Provider(OverlayStateProviderMode::Cached),
        )
    }

    const fn anchor_num_hash(anchor: &AnchorForParent) -> BlockNumHash {
        match anchor {
            AnchorForParent::NoReverts { anchor } |
            AnchorForParent::RevertsRequired { anchor, .. } => *anchor,
        }
    }

    #[reth_concurrency::test]
    fn builders_share_cached_overlays_across_snapshots() {
        // Finish is ahead of the durable trie; the tracked chain straddles both frontiers.
        let (factory, blocks) = setup_frontiers(1, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[2..=4].to_vec() });
        let first_view = context.snapshot_during(|| factory.provider()).transpose().unwrap();

        let child = TestBlockBuilder::eth().get_executed_block_with_number(
            blocks[4].block_number() + 1,
            blocks[4].recovered_block().hash(),
        );
        let child_hash = child.recovered_block().hash();
        in_memory_state_writer.insert_executed(child);
        let second_view = context.snapshot_during(|| factory.provider()).transpose().unwrap();
        assert!(first_view.snapshot().state_by_hash(child_hash).is_none());
        assert!(second_view.snapshot().state_by_hash(child_hash).is_some());

        // The oldest tracked block predates Finish and requires reverts.
        for index in [4usize, 3, 2] {
            let hash = blocks[index].recovered_block().hash();
            let first_builder = cached_builder(&first_view, hash);
            let second_builder = cached_builder(&second_view, hash);

            let anchor = anchor_num_hash(&first_builder.anchor_at_parent(&*first_view).unwrap());
            assert_eq!(
                anchor_num_hash(&second_builder.anchor_at_parent(&*second_view).unwrap()),
                anchor,
                "block {index} must resolve the same anchor from both views"
            );

            let (first_execution, first_fallback) =
                first_builder.execution_overlay(&*first_view).unwrap();
            let (second_execution, second_fallback) =
                second_builder.execution_overlay(&*second_view).unwrap();
            assert_eq!(first_fallback, second_fallback);
            assert!(
                Arc::ptr_eq(&first_execution, &second_execution),
                "block {index} must resolve the cached execution overlay"
            );

            let (first_nodes, first_state) =
                first_builder.resolve_state_trie_overlays(anchor.hash).unwrap();
            let (second_nodes, second_state) =
                second_builder.resolve_state_trie_overlays(anchor.hash).unwrap();
            assert!(
                Arc::ptr_eq(&first_nodes, &second_nodes),
                "block {index} must resolve the cached trie overlay nodes"
            );
            assert!(
                Arc::ptr_eq(&first_state, &second_state),
                "block {index} must resolve the cached trie overlay state"
            );

            // Reverts can require merging cached data into a new allocation.
            let first = first_builder.build_state_trie_overlay(&*first_view, false).unwrap();
            let second = second_builder.build_state_trie_overlay(&*second_view, false).unwrap();
            assert_eq!(account_keys(&first), account_keys(&second), "block {index}");
            assert_eq!(account_node_paths(&first), account_node_paths(&second), "block {index}");
        }
    }

    #[reth_concurrency::test]
    fn builder_resolves_snapshot_finish_chain_after_persistence_removes_it() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let parent = blocks[1].recovered_block().num_hash();
        let finish = blocks[3].recovered_block().num_hash();
        let incomplete = context.snapshot_during(|| factory.provider()).transpose().unwrap();
        let err = cached_builder(&incomplete, parent.hash)
            .build_state_trie_overlay(&*incomplete, false)
            .unwrap_err();
        assert_eq!(
            err.downcast_other_ref(),
            Some(&OverlayError::MissingFinishBlock { finish, trie_frontier: parent })
        );

        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[2..=4].to_vec() });
        let view = context.snapshot_during(|| factory.provider()).transpose().unwrap();
        let expected =
            cached_builder(&view, parent.hash).build_state_trie_overlay(&*view, false).unwrap();

        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .save_stage_checkpoint(StageId::Finish, StageCheckpoint::new(finish.number))
            .unwrap();
        provider_rw.commit().unwrap();
        in_memory_state_writer.on_blocks_persisted(finish, finish, None);
        let after = context.snapshot_during(|| factory.provider()).transpose().unwrap();
        assert!(after.snapshot().state_by_hash(finish.hash).is_none());
        assert_eq!(database_state_frontiers(&*after).unwrap(), (finish, finish));

        let overlay =
            cached_builder(&view, parent.hash).build_state_trie_overlay(&*view, false).unwrap();
        assert_same_overlay(&overlay, &expected);
    }

    #[reth_concurrency::test]
    fn builder_uses_its_snapshot_for_parent_lookup() {
        let blocks = test_blocks();
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let parent_hash = blocks[3].recovered_block().hash();
        let before = context.snapshot_during(|| ());

        in_memory_state_writer
            .update_chain(NewCanonicalChain::Commit { new: blocks[2..=4].to_vec() });
        let after = context.snapshot_during(|| ());

        assert!(cached_builder(&before, parent_hash).parent_state().is_none());
        assert_eq!(
            cached_builder(&after, parent_hash).parent_state(),
            after.snapshot().state_by_hash(parent_hash).as_ref()
        );
    }

    #[reth_concurrency::test]
    fn overlay_starts_at_state_trie_frontier() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        for block in &blocks[2..=4] {
            in_memory_state_writer.insert_executed(block.clone());
        }
        let provider = context.snapshot_during(|| factory.provider()).transpose().unwrap();

        for (parent_index, expected_ids) in [(3, vec![3, 4]), (4, vec![3, 4, 5])] {
            let overlay = cached_builder(&provider, blocks[parent_index].recovered_block().hash())
                .build_state_trie_overlay(&*provider, true)
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

    #[reth_concurrency::test]
    fn sparse_trie_reuse_skips_overlay_when_finish_is_the_anchor() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        in_memory_state_writer.insert_executed(blocks[4].clone());
        let provider = context.snapshot_during(|| factory.provider()).transpose().unwrap();

        let overlay = OverlayBuilder::new(
            blocks[4].recovered_block().hash(),
            OverlaySnapshot::from_view(&provider),
            OverlayBuildMode::ReusedSparseTrie { anchor_hash: blocks[3].recovered_block().hash() },
        )
        .build_state_trie_overlay(&*provider, true)
        .unwrap();

        assert!(overlay.input().state.is_empty());
        assert!(overlay.input().nodes.is_empty());
    }

    #[reth_concurrency::test]
    fn no_reverts_errors_when_reverts_are_required() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let provider = OverlayContext::<EthPrimitives>::default()
            .snapshot_during(|| factory.provider())
            .transpose()
            .unwrap();

        let builder = OverlayBuilder::new(
            blocks[1].recovered_block().hash(),
            OverlaySnapshot::from_view(&provider),
            OverlayBuildMode::Finish,
        );
        let error = builder.build_state_trie_overlay(&*provider, true).unwrap_err();

        assert_eq!(
            error.downcast_other_ref(),
            Some(&OverlayError::UnexpectedReverts {
                parent_hash: blocks[1].recovered_block().hash(),
                finish: blocks[3].recovered_block().num_hash(),
                anchor: blocks[1].recovered_block().num_hash(),
            })
        );
    }

    #[reth_concurrency::test]
    fn appended_overlay_rejects_noncanonical_anchor_when_reverts_are_required() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = OverlayContext::<EthPrimitives>::default()
            .snapshot_during(|| factory.provider())
            .transpose()
            .unwrap();
        let parent_hash = B256::with_last_byte(100);
        assert_ne!(parent_hash, blocks[1].recovered_block().hash());
        let block = TestBlockBuilder::eth()
            .get_executed_block_with_number(blocks[2].block_number(), parent_hash);
        let builder = OverlayBuilder::new(
            parent_hash,
            OverlaySnapshot::from_view(&provider),
            OverlayBuildMode::Provider(OverlayStateProviderMode::AppendBlock(block)),
        );

        assert!(matches!(
            builder.execution_overlay(&*provider),
            Err(ProviderError::BlockHashNotFound(hash)) if hash == parent_hash
        ));
        assert!(matches!(
            builder.build_state_trie_overlay(&*provider, true),
            Err(ProviderError::BlockHashNotFound(hash)) if hash == parent_hash
        ));
    }

    #[reth_concurrency::test]
    fn state_trie_overlay_invalidates_reverts_and_fork_without_trie_changesets() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let provider_rw = factory.provider_rw().unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                3,
                AccountBeforeTx {
                    address: Address::with_last_byte(1),
                    info: Some(Account::default()),
                },
            )
            .unwrap();
        provider_rw.commit().unwrap();

        // The fork changes keys distinct from those reverted on disk.
        let fork = TestBlockBuilder::eth()
            .get_executed_block_with_number(2, blocks[1].recovered_block().hash());
        let fork = with_unique_trie_data(&fork, 2);
        let provider = OverlayContext::<EthPrimitives>::default()
            .snapshot_during(|| factory.provider())
            .transpose()
            .unwrap();
        let overlay = OverlayBuilder::new(
            blocks[1].recovered_block().hash(),
            OverlaySnapshot::from_view(&provider),
            OverlayBuildMode::Provider(OverlayStateProviderMode::AppendBlock(fork)),
        )
        .build_state_trie_overlay(&*provider, false)
        .unwrap();

        // The fork's trie nodes are relative to the anchor, not to the Finish trie on disk.
        assert!(account_node_paths(&overlay).is_empty());

        let mut prefix_sets = overlay.input().prefix_sets.clone().freeze();
        assert!(prefix_sets
            .account_prefix_set
            .contains(&Nibbles::unpack(keccak256(Address::with_last_byte(1)))));
        assert!(prefix_sets.account_prefix_set.contains(&Nibbles::unpack(B256::with_last_byte(2))));
        assert!(prefix_sets
            .storage_prefix_sets
            .get_mut(&B256::with_last_byte(2))
            .unwrap()
            .contains(&Nibbles::unpack(B256::with_last_byte(34))));
    }

    #[reth_concurrency::test]
    fn execution_overlay_marks_historical_fallback() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider_rw = factory.provider_rw().unwrap();
        let address = Address::with_last_byte(1);
        let slot = U256::from(5);

        provider_rw
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                2,
                AccountBeforeTx {
                    address,
                    info: Some(Account { balance: U256::from(10), ..Default::default() }),
                },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                3,
                AccountBeforeTx {
                    address,
                    info: Some(Account { balance: U256::from(20), ..Default::default() }),
                },
            )
            .unwrap();
        for (block_number, value) in [(2, 10), (3, 15)] {
            provider_rw
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((block_number, address)),
                    StorageEntry { key: B256::from(slot), value: U256::from(value) },
                )
                .unwrap();
        }
        provider_rw.commit().unwrap();

        let provider = OverlayContext::<EthPrimitives>::default()
            .snapshot_during(|| factory.provider())
            .transpose()
            .unwrap();
        let (overlay, fallback_block_number) =
            cached_builder(&provider, blocks[1].recovered_block().hash())
                .execution_overlay(&*provider)
                .unwrap();

        assert_eq!(fallback_block_number, Some(2));
        assert!(overlay.accounts().is_empty());
        assert!(overlay.storage().is_empty());
        assert!(overlay.code_hashes().is_empty());
    }

    #[reth_concurrency::test]
    fn execution_overlay_uses_tracked_blocks_after_the_anchor() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        for block in &blocks[2..=4] {
            in_memory_state_writer.insert_executed(block.clone());
        }
        let provider = context.snapshot_during(|| factory.provider()).transpose().unwrap();

        let (overlay, fallback_block_number) =
            cached_builder(&provider, blocks[3].recovered_block().hash())
                .execution_overlay(&*provider)
                .unwrap();

        assert_eq!(fallback_block_number, None);

        for id in [3, 4] {
            let address = Address::with_last_byte(id);
            let slot = U256::from(id);
            assert_eq!(overlay.accounts()[&address].as_ref().unwrap().balance, U256::from(id));
            assert_eq!(overlay.storage()[&address][&slot], U256::from(id));
            assert!(overlay.code_hashes().contains_key(&B256::with_last_byte(id + 64)));
        }
        assert_eq!(
            overlay.block_hashes(),
            blocks[2..=3]
                .iter()
                .map(|block| block.recovered_block().num_hash())
                .collect::<Vec<_>>()
        );
    }

    #[reth_concurrency::test]
    fn execution_overlay_marks_historical_fallback_for_tracked_fork() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let address = Address::with_last_byte(1);
        let slot = U256::ONE;
        let provider_rw = factory.provider_rw().unwrap();
        for (block_number, balance, storage_value) in [(2u64, 10u64, 10u64), (3u64, 20u64, 15u64)] {
            provider_rw
                .tx_ref()
                .put::<tables::AccountChangeSets>(
                    block_number,
                    AccountBeforeTx {
                        address,
                        info: Some(Account { balance: U256::from(balance), ..Default::default() }),
                    },
                )
                .unwrap();
            provider_rw
                .tx_ref()
                .put::<tables::StorageChangeSets>(
                    BlockNumberAddress((block_number, address)),
                    StorageEntry { key: B256::from(slot), value: U256::from(storage_value) },
                )
                .unwrap();
        }
        provider_rw.commit().unwrap();

        let mut side_chain_builder = TestBlockBuilder::eth();
        let side_block_two = side_chain_builder.get_executed_block_with_number(
            blocks[2].block_number(),
            blocks[1].recovered_block().hash(),
        );
        let side_block_two = with_unique_trie_data(&side_block_two, 1);
        let side_block_three = side_chain_builder.get_executed_block_with_number(
            blocks[3].block_number(),
            side_block_two.recovered_block().hash(),
        );
        let side_block_three = with_unique_trie_data(&side_block_three, 1);
        assert_ne!(
            side_block_three.recovered_block().hash(),
            blocks[3].recovered_block().hash(),
            "the noncanonical chain must not contain the durable Finish block"
        );

        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        in_memory_state_writer.insert_executed(side_block_two.clone());
        in_memory_state_writer.insert_executed(side_block_three.clone());
        let provider = context.snapshot_during(|| factory.provider()).transpose().unwrap();

        let (overlay, fallback_block_number) =
            cached_builder(&provider, side_block_three.recovered_block().hash())
                .execution_overlay(&*provider)
                .unwrap();

        assert_eq!(fallback_block_number, Some(2));

        assert_eq!(overlay.accounts()[&address].as_ref().unwrap().balance, U256::ONE);
        assert_eq!(overlay.storage()[&address][&slot], U256::ONE);
        assert_eq!(
            overlay.block_hashes(),
            [side_block_two, side_block_three]
                .iter()
                .map(|block| block.recovered_block().num_hash())
                .collect::<Vec<_>>()
        );
    }

    #[reth_concurrency::test]
    fn overlay_uses_persisted_parent_even_if_retained() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        in_memory_state_writer.insert_executed(blocks[1].clone());
        let provider = context.snapshot_during(|| factory.provider()).transpose().unwrap();
        let builder = cached_builder(&provider, blocks[1].recovered_block().hash());
        match builder.anchor_at_parent(&*provider).unwrap() {
            AnchorForParent::RevertsRequired { anchor, finish } => {
                assert_eq!(anchor, blocks[1].recovered_block().num_hash());
                assert_eq!(finish, blocks[3].recovered_block().num_hash());
            }
            AnchorForParent::NoReverts { .. } => {
                panic!("persisted parent below Finish must require reverts")
            }
        }
    }

    #[reth_concurrency::test]
    fn builder_appends_block_to_parent_state() {
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        let blocks = test_blocks();
        for block in &blocks[2..=4] {
            in_memory_state_writer.insert_executed(block.clone());
        }

        let block = TestBlockBuilder::eth().get_executed_block_with_number(
            blocks[4].recovered_block().number() + 1,
            blocks[4].recovered_block().hash(),
        );
        let view = context.snapshot_during(|| ());
        let builder = OverlayBuilder::new(
            block.recovered_block().parent_hash(),
            OverlaySnapshot::from_view(&view),
            OverlayBuildMode::Provider(OverlayStateProviderMode::AppendBlock(block.clone())),
        );

        assert_eq!(builder.parent_hash, block.recovered_block().hash());
        assert_eq!(
            builder.parent_state.unwrap().chain().map(BlockState::hash).collect::<Vec<_>>(),
            std::iter::once(&block)
                .chain(blocks[2..=4].iter().rev())
                .map(|block| block.recovered_block().hash())
                .collect::<Vec<_>>(),
        );
    }

    #[test]
    #[should_panic(expected = "appended block must extend the requested parent")]
    fn builder_rejects_a_block_appended_to_the_wrong_parent() {
        let view = OverlayContext::<EthPrimitives>::default().snapshot_during(|| ());
        let parent_hash = B256::with_last_byte(1);
        let block =
            TestBlockBuilder::eth().get_executed_block_with_number(1, B256::with_last_byte(2));
        OverlayBuilder::new(
            parent_hash,
            OverlaySnapshot::from_view(&view),
            OverlayBuildMode::Provider(OverlayStateProviderMode::AppendBlock(block)),
        );
    }

    #[reth_concurrency::test]
    fn overlay_after_state_trie_frontier_requires_in_memory_coverage() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = OverlayContext::<EthPrimitives>::default()
            .snapshot_during(|| factory.provider())
            .transpose()
            .unwrap();
        let error = cached_builder(&provider, blocks[3].recovered_block().hash())
            .build_state_trie_overlay(&*provider, true)
            .unwrap_err();

        assert_eq!(
            error.downcast_other_ref(),
            Some(&OverlayError::AnchorAfterTrieFrontier {
                anchor: blocks[3].recovered_block().num_hash(),
                trie_frontier: blocks[1].recovered_block().num_hash(),
            })
        );
    }

    #[reth_concurrency::test]
    fn persisted_parent_has_empty_overlay() {
        let parent_hash = B256::with_last_byte(1);
        let view = OverlayContext::<EthPrimitives>::default().snapshot_during(|| ());
        let builder = cached_builder(&view, parent_hash);

        let (trie, state) = builder.resolve_state_trie_overlays(parent_hash).unwrap();
        assert!(trie.is_empty());
        assert!(state.is_empty());
    }

    #[reth_concurrency::test]
    fn overlay_errors_if_parent_is_not_persisted_or_tracked() {
        let parent_hash = B256::with_last_byte(1);
        let anchor_hash = B256::with_last_byte(2);
        let view = OverlayContext::<EthPrimitives>::default().snapshot_during(|| ());
        let builder = cached_builder(&view, parent_hash);

        let err = builder.resolve_state_trie_overlays(anchor_hash).unwrap_err();

        assert_eq!(err.downcast_other_ref(), Some(&OverlayError::MissingParent(parent_hash)));
    }

    #[reth_concurrency::test]
    fn sparse_trie_reuse_requires_both_frontiers() {
        let parent_hash = B256::with_last_byte(1);
        let view = OverlayContext::<EthPrimitives>::default().snapshot_during(|| ());
        let builder = cached_builder(&view, parent_hash);
        assert!(!builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));

        let builder = OverlayBuilder::new(
            parent_hash,
            OverlaySnapshot::from_view(&view),
            OverlayBuildMode::ReusedSparseTrie { anchor_hash: parent_hash },
        );
        assert!(builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));
        assert!(!builder
            .should_skip_overlay_for_reused_sparse_trie(B256::with_last_byte(3), parent_hash,));

        let blocks = test_blocks();
        let (context, mut in_memory_state_writer) = overlay_context_with_writer();
        for block in &blocks[2..=4] {
            in_memory_state_writer.insert_executed(block.clone());
        }
        let view = context.snapshot_during(|| ());
        let builder = OverlayBuilder::new(
            blocks[4].recovered_block().hash(),
            OverlaySnapshot::from_view(&view),
            OverlayBuildMode::ReusedSparseTrie { anchor_hash: blocks[1].recovered_block().hash() },
        );
        assert!(builder.should_skip_overlay_for_reused_sparse_trie(
            blocks[1].recovered_block().hash(),
            blocks[3].recovered_block().hash(),
        ));

        let builder = OverlayBuilder::new(
            blocks[4].recovered_block().hash(),
            OverlaySnapshot::from_view(&view),
            OverlayBuildMode::ReusedSparseTrie { anchor_hash: blocks[2].recovered_block().hash() },
        );
        assert!(!builder.should_skip_overlay_for_reused_sparse_trie(
            blocks[1].recovered_block().hash(),
            blocks[3].recovered_block().hash(),
        ));
    }
}
