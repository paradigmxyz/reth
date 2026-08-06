use crate::OverlayManager;
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{AddressMap, B256Map, U256Map},
    BlockHash, BlockNumber, B256, U256,
};
use metrics::{Counter, Histogram};
use reth_chain_state::ExecutedBlock;
use reth_errors::{ProviderError, ProviderResult};
use reth_ethereum_primitives::EthPrimitives;
use reth_metrics::Metrics;
use reth_primitives_traits::{AlloyBlockHeader, NodePrimitives};
use reth_prune_types::PruneSegment;
use reth_stages_types::StageId;
use reth_storage_api::{
    BlockNumReader, ChangeSetReader, DBProvider, PruneCheckpointReader, StageCheckpointReader,
    StorageChangeSetReader, StorageSettingsCache,
};
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted};
use reth_trie_db::DatabaseHashedPostState;
use revm::{bytecode::Bytecode, state::AccountInfo};
use std::{
    collections::HashSet,
    ops::RangeInclusive,
    sync::Arc,
    time::{Duration, Instant},
};
use tracing::{debug, debug_span, instrument};

/// Contains the trie and hashed-state data required to initialize an overlay state provider.
#[derive(Debug, Clone)]
pub struct StateTrieOverlay {
    /// Trie updates overlay.
    pub trie_updates: Arc<TrieUpdatesSorted>,
    /// Hashed state overlay.
    pub hashed_post_state: Arc<HashedPostStateSorted>,
}

impl StateTrieOverlay {
    fn empty() -> Self {
        Self {
            trie_updates: Arc::new(TrieUpdatesSorted::default()),
            hashed_post_state: Arc::new(HashedPostStateSorted::default()),
        }
    }
}

/// Execution state required to initialize an overlay state provider.
///
/// Account entries preserve known non-existence, while storage and code entries contain only data
/// explicitly observed during execution.
#[derive(Clone, Debug, Default)]
pub struct ExecutionOverlay {
    /// In-memory block hashes in ascending block-number order.
    pub block_hashes: Vec<BlockNumHash>,
    /// Account state by address.
    pub accounts: AddressMap<Option<AccountInfo>>,
    /// Storage values by address and slot.
    pub storage: AddressMap<U256Map<U256>>,
    /// Bytecode by code hash.
    pub code_hashes: B256Map<Bytecode>,
}

/// Source of data to apply on top of the durable database state.
#[derive(Debug, Clone)]
pub enum OverlaySource {
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
    Managed,
}

/// Builder for calculating trie and hashed-state overlays.
///
/// This stores the overlay manager, overlay configuration, and the logic for resolving overlays
/// and collecting reverts.
#[derive(Debug, Clone)]
pub struct OverlayBuilder<N: NodePrimitives = EthPrimitives> {
    /// Parent hash requested by the caller.
    parent_hash: B256,
    /// Optional overlay source.
    overlay_source: Option<OverlaySource>,
    /// Manager used for cached changesets and in-memory parent state.
    overlay_manager: OverlayManager<N>,
    /// Anchor hash of the reused sparse trie, if this task reused one.
    reused_sparse_trie_anchor_hash: Option<B256>,
    /// Whether building the overlay may query revert changesets.
    no_reverts: bool,
    /// Metrics for overlay construction.
    metrics: OverlayBuilderMetrics,
}

impl<N: NodePrimitives> OverlayBuilder<N> {
    /// Create a new manager-backed overlay builder.
    pub(crate) fn new(parent_hash: B256, overlay_manager: OverlayManager<N>) -> Self {
        Self {
            parent_hash,
            overlay_source: Some(OverlaySource::Managed),
            overlay_manager,
            reused_sparse_trie_anchor_hash: None,
            no_reverts: false,
            metrics: OverlayBuilderMetrics::default(),
        }
    }

    /// Set the overlay source.
    ///
    /// This overlay will be applied on top of any reverts.
    pub fn with_overlay_source(mut self, source: Option<OverlaySource>) -> Self {
        self.overlay_source = source;
        self
    }

    /// Skips managed overlay construction when the sparse trie was reused and the DB tip is
    /// already covered by its anchor-to-parent range.
    pub const fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.reused_sparse_trie_anchor_hash = Some(anchor_hash);
        self
    }

    /// Returns an error instead of querying revert changesets when reverts are required.
    pub const fn with_no_reverts(mut self) -> Self {
        self.no_reverts = true;
        self
    }

    /// Sets an immediate hashed-state and trie-updates overlay.
    pub fn with_immediate_state_trie_overlay(
        mut self,
        state: Arc<HashedPostStateSorted>,
        trie: Arc<TrieUpdatesSorted>,
    ) -> Self {
        self.overlay_source = Some(OverlaySource::Immediate { trie, state });
        self
    }

    /// Returns the durable anchor to use for this builder's parent.
    pub fn anchor_at_parent<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<AnchorForParent<N>>
    where
        Provider: StageCheckpointReader + BlockNumReader + PruneCheckpointReader,
    {
        let (partial_state_trie, finish) = database_state_frontiers(provider)?;
        self.anchor_at_parent_with_frontiers(provider, partial_state_trie, finish)
    }

    /// Returns the durable anchor to use for this builder's parent using known frontiers.
    pub fn anchor_at_parent_with_frontiers<Provider>(
        &self,
        provider: &Provider,
        partial_state_trie: BlockNumHash,
        finish: BlockNumHash,
    ) -> ProviderResult<AnchorForParent<N>>
    where
        Provider: BlockNumReader + PruneCheckpointReader,
    {
        match &self.overlay_source {
            Some(OverlaySource::Managed) => anchor_for_parent_with_frontiers(
                self.parent_hash,
                self.overlay_manager.parent_chain(self.parent_hash),
                partial_state_trie,
                finish,
                provider,
            ),
            _ => anchor_for_parent_with_frontiers(
                self.parent_hash,
                std::iter::empty(),
                partial_state_trie,
                finish,
                provider,
            ),
        }
    }

    /// Builds the effective state trie overlay for the given provider.
    #[instrument(level = "debug", target = "storage::overlay", skip_all)]
    pub fn build_state_trie_overlay<Provider>(
        &self,
        provider: &Provider,
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
        self.build_state_trie_overlay_at_frontiers(provider, state_trie_tip_block, finish_tip_block)
    }

    /// Builds the effective state trie overlay using frontiers already read from the provider.
    ///
    /// This is useful for callers that key an overlay cache by the durable frontiers.
    #[instrument(
        level = "debug",
        target = "storage::overlay",
        skip_all,
        fields(?state_trie_tip_block, ?finish_tip_block, parent_hash = ?self.parent_hash)
    )]
    pub fn build_state_trie_overlay_at_frontiers<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<StateTrieOverlay>
    where
        Provider: ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
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
        let (trie_updates, hashed_post_state) = match &anchor_for_parent {
            AnchorForParent::RevertsRequired { anchor, .. } => {
                let revert_blocks =
                    self.revert_blocks(&anchor_for_parent)?.expect("reverts are required");

                debug!(
                    target: "storage::overlay",
                    ?revert_blocks,
                    ?anchor,
                    "Collecting trie reverts for overlay state provider"
                );

                let trie_reverts = {
                    let _guard = debug_span!(target: "storage::overlay", "retrieving_trie_reverts")
                        .entered();
                    let start = Instant::now();
                    let accumulated_reverts =
                        self.overlay_manager.get_or_compute_cached_changesets_range_at_frontiers(
                            provider,
                            revert_blocks.clone(),
                            state_trie_tip_block,
                            finish_tip_block,
                        )?;
                    retrieve_trie_reverts_duration = start.elapsed();
                    accumulated_reverts
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
                    target: "storage::overlay",
                    num_trie_updates = ?trie_updates_total_len,
                    num_state_updates = ?hashed_state_updates_total_len,
                    ?anchor,
                    "Reverted to anchor block",
                );

                (trie_updates, hashed_state_updates)
            }
            AnchorForParent::NoReverts { anchor, .. } => {
                // If no reverts are needed, use the manager overlay directly unless the reused
                // sparse trie already covers both durable frontiers through the
                // requested parent.
                if self.should_skip_overlay_for_reused_sparse_trie(
                    state_trie_tip_block.hash,
                    finish_tip_block.hash,
                ) {
                    debug!(
                        target: "storage::overlay",
                        parent_hash = %self.parent_hash,
                        state_trie_tip_hash = %state_trie_tip_block.hash,
                        finish_tip_hash = %finish_tip_block.hash,
                        sparse_trie_anchor_hash = ?self.reused_sparse_trie_anchor_hash,
                        "Skipping overlay construction because reused sparse trie covers durable frontiers to parent"
                    );

                    self.metrics.sparse_trie_overlay_skips.increment(1);

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

                (trie_updates, hashed_post_state)
            }
        };

        self.metrics
            .retrieve_trie_reverts_duration
            .record(retrieve_trie_reverts_duration.as_secs_f64());
        self.metrics
            .retrieve_hashed_state_reverts_duration
            .record(retrieve_hashed_state_reverts_duration.as_secs_f64());
        self.metrics.trie_updates_size.record(trie_updates_total_len as f64);
        self.metrics.hashed_state_size.record(hashed_state_updates_total_len as f64);

        Ok(StateTrieOverlay { trie_updates, hashed_post_state })
    }

    /// Builds the effective execution overlay for the given provider.
    #[instrument(level = "debug", target = "storage::overlay", skip_all)]
    pub fn build_execution_overlay<Provider>(
        &self,
        provider: &Provider,
    ) -> ProviderResult<ExecutionOverlay>
    where
        Provider: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + BlockNumReader,
    {
        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(provider)?;
        self.build_execution_overlay_at_frontiers(provider, state_trie_tip_block, finish_tip_block)
    }

    /// Builds the effective execution overlay using frontiers already read from the provider.
    #[instrument(
        level = "debug",
        target = "storage::overlay",
        skip_all,
        fields(?state_trie_tip_block, ?finish_tip_block, parent_hash = ?self.parent_hash)
    )]
    pub fn build_execution_overlay_at_frontiers<Provider>(
        &self,
        provider: &Provider,
        state_trie_tip_block: BlockNumHash,
        finish_tip_block: BlockNumHash,
    ) -> ProviderResult<ExecutionOverlay>
    where
        Provider: ChangeSetReader + StorageChangeSetReader + BlockNumReader + PruneCheckpointReader,
    {
        let anchor_for_parent =
            self.anchor_at_parent_with_frontiers(provider, state_trie_tip_block, finish_tip_block)?;
        let anchor = anchor_for_parent.anchor();
        let mut overlay = match self.revert_blocks(&anchor_for_parent)? {
            Some(revert_blocks) => {
                debug!(
                    target: "storage::overlay",
                    ?revert_blocks,
                    ?anchor,
                    "Collecting execution reverts for overlay state provider"
                );
                execution_reverts(provider, revert_blocks)?
            }
            None => ExecutionOverlay::default(),
        };

        let managed_overlay = self.resolve_execution_overlay(anchor.hash)?;
        overlay.block_hashes.extend_from_slice(&managed_overlay.block_hashes);
        overlay.accounts.extend(
            managed_overlay.accounts.iter().map(|(address, info)| (*address, info.clone())),
        );
        for (address, slots) in &managed_overlay.storage {
            overlay
                .storage
                .entry(*address)
                .or_default()
                .extend(slots.iter().map(|(slot, value)| (*slot, *value)));
        }
        overlay
            .code_hashes
            .extend(managed_overlay.code_hashes.iter().map(|(hash, code)| (*hash, code.clone())));
        Ok(overlay)
    }

    /// Resolves the effective overlay (trie updates, hashed state).
    fn resolve_state_trie_overlays(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<(Arc<TrieUpdatesSorted>, Arc<HashedPostStateSorted>)> {
        match &self.overlay_source {
            Some(OverlaySource::Managed) => {
                if anchor_hash == self.parent_hash {
                    Ok((
                        Arc::new(TrieUpdatesSorted::default()),
                        Arc::new(HashedPostStateSorted::default()),
                    ))
                } else {
                    self.overlay_manager
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

    /// Resolves the execution overlay for the configured in-memory source.
    fn resolve_execution_overlay(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<ExecutionOverlay> {
        match &self.overlay_source {
            Some(OverlaySource::Managed) if anchor_hash != self.parent_hash => self
                .overlay_manager
                .execution_overlay_for_parent(self.parent_hash, anchor_hash)
                .map(|overlay| (*overlay).clone())
                .map_err(ProviderError::other),
            Some(OverlaySource::Immediate { .. }) if anchor_hash != self.parent_hash => {
                Err(ProviderError::other(std::io::Error::other(format!(
                    "anchor_hash {anchor_hash} doesn't match OverlayBuilder's configured parent ({})",
                    self.parent_hash
                ))))
            }
            _ => Ok(ExecutionOverlay::default()),
        }
    }

    /// Returns the blocks to revert from Finish to the selected anchor, if any.
    fn revert_blocks(
        &self,
        anchor_for_parent: &AnchorForParent<N>,
    ) -> ProviderResult<Option<RangeInclusive<BlockNumber>>> {
        match anchor_for_parent {
            AnchorForParent::NoReverts { .. } => Ok(None),
            AnchorForParent::RevertsRequired { anchor, finish, .. } => {
                if self.no_reverts {
                    return Err(ProviderError::other(std::io::Error::other(format!(
                        "reverts are disabled, but overlay for parent {} requires reverting Finish #{} ({}) to anchor #{} ({})",
                        self.parent_hash, finish.number, finish.hash, anchor.number, anchor.hash,
                    ))))
                }
                Ok(Some(anchor.number + 1..=finish.number))
            }
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
            Some(OverlaySource::Managed) => {
                self.overlay_manager.contains_hash(
                    self.parent_hash,
                    anchor_hash,
                    state_trie_tip_hash,
                ) && self.overlay_manager.contains_hash(
                    self.parent_hash,
                    anchor_hash,
                    finish_tip_hash,
                )
            }
            _ => false,
        }
    }
}

/// Returns the highest blocks whose state/trie data and non-state/trie data are durably
/// available in the database.
pub fn database_state_frontiers<Provider>(
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

/// Metrics for overlay construction.
#[derive(Clone, Metrics)]
#[metrics(scope = "storage.overlay.builder")]
struct OverlayBuilderMetrics {
    /// Duration of retrieving trie updates from the database.
    retrieve_trie_reverts_duration: Histogram,
    /// Duration of retrieving hashed state from the database.
    retrieve_hashed_state_reverts_duration: Histogram,
    /// Size of trie updates (number of entries).
    trie_updates_size: Histogram,
    /// Size of hashed state (number of entries).
    hashed_state_size: Histogram,
    /// Number of managed overlay creations skipped because the reused sparse trie already covers
    /// the DB tip to parent range.
    sparse_trie_overlay_skips: Counter,
}

fn anchor_for_parent_in<N: NodePrimitives>(
    parent_hash: B256,
    mut in_mem_chain: impl Iterator<Item = ExecutedBlock<N>>,
    preferred_anchor: B256,
) -> B256 {
    if parent_hash == preferred_anchor {
        return parent_hash
    }

    let mut hash = parent_hash;

    loop {
        let Some(block) = in_mem_chain.next() else { return hash };
        let block_parent_hash = block.recovered_block().parent_hash();

        if block_parent_hash == preferred_anchor {
            return block_parent_hash
        }
        hash = block_parent_hash;
    }
}

/// Describes whether an overlay must revert the database before using its anchor.
#[derive(Debug)]
pub enum AnchorForParent<N: NodePrimitives> {
    /// The in-memory chain covers the durable frontiers through this anchor.
    NoReverts {
        /// Block to anchor the overlay to.
        anchor: BlockNumHash,
        /// In-memory blocks from `parent_hash` through, but excluding, `anchor`.
        overlay: Vec<ExecutedBlock<N>>,
    },
    /// The database must be reverted from `finish` to `anchor` first.
    RevertsRequired {
        /// Block to anchor the overlay to.
        anchor: BlockNumHash,
        /// Current Finish frontier.
        finish: BlockNumHash,
        /// In-memory blocks from `parent_hash` through, but excluding, `anchor`.
        overlay: Vec<ExecutedBlock<N>>,
    },
}

impl<N: NodePrimitives> AnchorForParent<N> {
    /// Returns the durable block anchoring this overlay.
    pub const fn anchor(&self) -> BlockNumHash {
        match self {
            Self::NoReverts { anchor, .. } | Self::RevertsRequired { anchor, .. } => *anchor,
        }
    }
}

/// Collects the execution data needed to revert the database to `range`'s first block.
fn execution_reverts<Provider>(
    provider: &Provider,
    range: RangeInclusive<BlockNumber>,
) -> ProviderResult<ExecutionOverlay>
where
    Provider: ChangeSetReader + StorageChangeSetReader,
{
    let mut overlay = ExecutionOverlay::default();
    let mut seen_accounts = HashSet::new();
    for (_, account) in provider.account_changesets_range(range.clone())? {
        if seen_accounts.insert(account.address) {
            overlay.accounts.insert(account.address, account.info.map(Into::into));
        }
    }

    let mut seen_storage = HashSet::new();
    for (block_address, storage) in provider.storage_changesets_range(range)? {
        let address = block_address.address();
        if seen_storage.insert((address, storage.key)) {
            overlay
                .storage
                .entry(address)
                .or_default()
                .insert(U256::from_be_bytes(storage.key.0), storage.value);
        }
    }

    // Bytecode rows are append-only. Restoring an account's previous code hash therefore makes
    // its bytecode directly available from the database without an additional revert overlay.
    Ok(overlay)
}

/// Returns the anchor block to use for the target parent and a chain of in-memory blocks.
///
/// # Arguments
/// * `parent`: The block whose post-state is being targeted.
/// * `in_mem_chain`: Yields the in-memory blocks in the chain, starting at `parent_hash`.
/// * `provider`: Used to resolve the durable frontiers and check changeset availability.
pub fn anchor_for_parent<N, Provider>(
    parent_hash: B256,
    in_mem_chain: impl Iterator<Item = ExecutedBlock<N>>,
    provider: &Provider,
) -> ProviderResult<AnchorForParent<N>>
where
    N: NodePrimitives,
    Provider: StageCheckpointReader + BlockNumReader + PruneCheckpointReader,
{
    let (partial_state_trie, finish) = database_state_frontiers(provider)?;
    anchor_for_parent_with_frontiers(
        parent_hash,
        in_mem_chain,
        partial_state_trie,
        finish,
        provider,
    )
}

/// Returns the anchor block to use for the target parent and a chain of in-memory blocks using
/// known durable frontiers.
///
/// # Arguments
/// * `parent`: The block whose post-state is being targeted.
/// * `in_mem_chain`: Yields the in-memory blocks in the chain, starting at `parent_hash`.
/// * `partial_state_trie`: The durable state/trie frontier.
/// * `finish`: The durable Finish frontier.
/// * `provider`: Used to resolve the parent and check changeset availability.
pub fn anchor_for_parent_with_frontiers<N, Provider>(
    parent_hash: B256,
    in_mem_chain: impl Iterator<Item = ExecutedBlock<N>>,
    partial_state_trie: BlockNumHash,
    finish: BlockNumHash,
    provider: &Provider,
) -> ProviderResult<AnchorForParent<N>>
where
    N: NodePrimitives,
    Provider: BlockNumReader + PruneCheckpointReader,
{
    use std::io::Error;

    let persisted_parent = provider
        .block_number(parent_hash)?
        .filter(|&parent_number| parent_number <= partial_state_trie.number);

    let mut finish_seen = parent_hash == finish.hash;
    let (anchor, overlay) = if let Some(parent_number) = persisted_parent {
        (BlockNumHash::new(parent_number, parent_hash), Vec::new())
    } else {
        let mut overlay = Vec::new();
        let mut in_mem_chain = in_mem_chain.inspect(|block| {
            finish_seen |= block.recovered_block().hash() == finish.hash;
            overlay.push(block.clone());
        });

        let anchor_hash =
            anchor_for_parent_in(parent_hash, &mut in_mem_chain, partial_state_trie.hash);
        let anchor = if anchor_hash == partial_state_trie.hash {
            BlockNumHash::new(partial_state_trie.number, anchor_hash)
        } else {
            let anchor_number = provider
                .convert_hash_or_number(anchor_hash.into())?
                .ok_or(ProviderError::BlockHashNotFound(anchor_hash))?;
            BlockNumHash::new(anchor_number, anchor_hash)
        };
        (anchor, overlay)
    };

    finish_seen |= anchor.hash == finish.hash;

    if anchor.number > partial_state_trie.number {
        return Err(ProviderError::other(Error::other(format!(
                "overlay anchor #{} ({}) is after partial state trie frontier #{} ({}); missing trie updates for blocks #{}..=#{}",
                anchor.number,
                anchor.hash,
                partial_state_trie.number,
                partial_state_trie.hash,
                partial_state_trie.number + 1,
                anchor.number,
            ))))
    }

    // If the Finish block (db tip) was seen in the in-memory chain then we know that anchor is on
    // the same chain as partial_state_trie as well. Given that anchor <= partial_state_trie, we can
    // be sure that the in-memory chain is a superset of partial_state_trie+1..finish, and therefore
    // can be used without reverts.
    if finish_seen {
        return Ok(AnchorForParent::NoReverts { anchor, overlay })
    }

    // Otherwise reverts are required; we check the changesets to make sure they are actually
    // available before signaling that they are required.
    let account_history = provider
        .get_prune_checkpoint(PruneSegment::AccountHistory)?
        .and_then(|checkpoint| checkpoint.block_number);
    let storage_history = provider
        .get_prune_checkpoint(PruneSegment::StorageHistory)?
        .and_then(|checkpoint| checkpoint.block_number);
    let lower_bound = account_history.max(storage_history).unwrap_or_default();
    let available_range = lower_bound..=finish.number;
    if !available_range.contains(&anchor.number) {
        return Err(ProviderError::InsufficientChangesets {
            requested: anchor.number,
            available: available_range,
        })
    }

    Ok(AnchorForParent::RevertsRequired { anchor, finish, overlay })
}

#[cfg(test)]
mod tests {
    use super::*;
    use alloy_primitives::{map::HashMap, Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    #[cfg(feature = "partial-persistence")]
    use reth_db::{
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::Account;
    #[cfg(feature = "partial-persistence")]
    use reth_primitives_traits::StorageEntry;
    #[cfg(feature = "partial-persistence")]
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    #[cfg(feature = "partial-persistence")]
    #[cfg(feature = "partial-persistence")]
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint};
    #[cfg(feature = "partial-persistence")]
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{BranchNodeCompact, ComputedTrieData, HashedPostState, HashedStorage, Nibbles};
    use revm::{bytecode::Bytecode, database::BundleState, state::AccountInfo};

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
        let address = Address::with_last_byte(id);
        let slot = U256::from(id);
        let code_hash = B256::with_last_byte(id.saturating_add(64));
        let state = BundleState::builder(block.block_number()..=block.block_number())
            .state_present_account_info(
                address,
                AccountInfo { nonce: id as u64, balance: U256::from(id), ..Default::default() },
            )
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, U256::from(id)))]))
            .contract(code_hash, Bytecode::new_raw(vec![id].into()))
            .build();
        let mut execution_output = (*block.execution_output).clone();
        execution_output.state = state;

        ExecutedBlock::new(
            Arc::clone(&block.recovered_block),
            Arc::new(execution_output),
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
    fn account_keys(overlay: &StateTrieOverlay) -> Vec<B256> {
        overlay.hashed_post_state.accounts.iter().map(|(key, _)| *key).collect()
    }

    #[cfg(feature = "partial-persistence")]
    fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
        overlay.trie_updates.account_nodes_ref().iter().map(|(path, _)| *path).collect()
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn managed_overlay_starts_at_state_trie_frontier() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let provider = factory.provider().unwrap();

        for (parent_index, expected_ids) in [(3, vec![3, 4]), (4, vec![3, 4, 5])] {
            let overlay = manager
                .overlay_builder(blocks[parent_index].recovered_block().hash())
                .build_state_trie_overlay(&provider)
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
    fn managed_overlay_skips_when_finish_is_the_anchor() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[4].clone());
        let provider = factory.provider().unwrap();

        let overlay = manager
            .overlay_builder(blocks[4].recovered_block().hash())
            .with_skip_overlay_for_reused_sparse_trie(blocks[3].recovered_block().hash())
            .build_state_trie_overlay(&provider)
            .unwrap();

        assert!(overlay.hashed_post_state.is_empty());
        assert!(overlay.trie_updates.is_empty());
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn no_reverts_errors_when_reverts_are_required() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let provider = factory.provider().unwrap();

        let builder = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[1].recovered_block().hash())
            .with_no_reverts();
        let error = builder.build_state_trie_overlay(&provider).unwrap_err();

        assert!(error.to_string().contains("reverts are disabled"));
        let error = builder.build_execution_overlay(&provider).unwrap_err();
        assert!(error.to_string().contains("reverts are disabled"));
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn execution_overlay_reverts_to_anchor_state() {
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

        let provider = factory.provider().unwrap();
        let overlay = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[1].recovered_block().hash())
            .build_execution_overlay(&provider)
            .unwrap();

        assert_eq!(overlay.accounts[&address].as_ref().unwrap().balance, U256::from(10));
        assert_eq!(overlay.storage[&address][&slot], U256::from(10));
        assert!(overlay.code_hashes.is_empty());
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn execution_overlay_uses_managed_blocks_after_the_anchor() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let provider = factory.provider().unwrap();

        let overlay = manager
            .overlay_builder(blocks[3].recovered_block().hash())
            .build_execution_overlay(&provider)
            .unwrap();

        for id in [3, 4] {
            let address = Address::with_last_byte(id);
            let slot = U256::from(id);
            assert_eq!(overlay.accounts[&address].as_ref().unwrap().balance, U256::from(id));
            assert_eq!(overlay.storage[&address][&slot], U256::from(id));
            assert!(overlay.code_hashes.contains_key(&B256::with_last_byte(id + 64)));
        }
        assert_eq!(
            overlay.block_hashes,
            blocks[2..=3]
                .iter()
                .map(|block| block.recovered_block().num_hash())
                .collect::<Vec<_>>()
        );
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn managed_overlay_uses_persisted_parent_even_if_retained() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[1].clone());
        let provider = factory.provider().unwrap();
        let builder = manager.overlay_builder(blocks[1].recovered_block().hash());
        match builder.anchor_at_parent(&provider).unwrap() {
            AnchorForParent::RevertsRequired { anchor, finish, overlay } => {
                assert_eq!(anchor, blocks[1].recovered_block().num_hash());
                assert_eq!(finish, blocks[3].recovered_block().num_hash());
                assert!(overlay.is_empty());
            }
            AnchorForParent::NoReverts { .. } => {
                panic!("persisted parent below Finish must require reverts")
            }
        }

        let overlay = builder.build_state_trie_overlay(&provider).unwrap();

        assert!(overlay.hashed_post_state.is_empty());
        assert!(overlay.trie_updates.is_empty());
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn overlay_after_state_trie_frontier_requires_managed_coverage() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let error = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[3].recovered_block().hash())
            .with_overlay_source(None)
            .build_state_trie_overlay(&provider)
            .unwrap_err();

        assert!(
            error.to_string().contains("is after partial state trie frontier"),
            "unexpected error: {error}"
        );
    }

    #[cfg(feature = "partial-persistence")]
    #[test]
    fn managed_overlay_errors_if_parent_is_not_persisted_or_managed_across_frontiers() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let parent_hash = blocks[3].recovered_block().hash();
        let error = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(parent_hash)
            .build_state_trie_overlay(&provider)
            .unwrap_err();

        assert!(error.to_string().contains("is after partial state trie frontier"));
    }

    #[test]
    fn managed_overlay_skips_manager_for_persisted_parent() {
        let parent_hash = B256::with_last_byte(1);
        let builder = OverlayManager::<EthPrimitives>::default().overlay_builder(parent_hash);

        let (trie, state) = builder.resolve_state_trie_overlays(parent_hash).unwrap();
        assert!(trie.is_empty());
        assert!(state.is_empty());
    }

    #[test]
    fn managed_overlay_errors_if_parent_is_not_persisted_or_managed() {
        let parent_hash = B256::with_last_byte(1);
        let anchor_hash = B256::with_last_byte(2);
        let builder = OverlayManager::<EthPrimitives>::default().overlay_builder(parent_hash);

        let err = builder.resolve_state_trie_overlays(anchor_hash).unwrap_err();

        assert!(err.to_string().contains("cannot be anchored"));
    }

    #[test]
    fn managed_overlay_skip_requires_both_frontiers() {
        let parent_hash = B256::with_last_byte(1);
        let builder = OverlayManager::<EthPrimitives>::default().overlay_builder(parent_hash);
        assert!(!builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));

        let builder = builder.with_skip_overlay_for_reused_sparse_trie(parent_hash);
        assert!(builder.should_skip_overlay_for_reused_sparse_trie(parent_hash, parent_hash));
        assert!(!builder
            .should_skip_overlay_for_reused_sparse_trie(B256::with_last_byte(3), parent_hash,));

        let blocks = test_blocks();
        let manager = OverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let builder = manager
            .overlay_builder(blocks[4].recovered_block().hash())
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
