use crate::{manager::OverlayCacheConfig, OverlayManager};
use alloy_eips::BlockNumHash;
use alloy_primitives::{
    map::{AddressMap, AddressSet, B256Map, U256Map},
    Address, BlockHash, BlockNumber, B256, U256,
};
use metrics::{Counter, Histogram};
use reth_chain_state::{BlockState, ExecutedBlock};
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
use reth_trie::{updates::TrieUpdatesSorted, HashedPostStateSorted, TrieInputSorted};
use reth_trie_db::DatabaseHashedPostState;
use revm::{bytecode::Bytecode, database::BundleState, state::AccountInfo};
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

    /// Consumes the overlay and returns its trie input.
    pub fn into_input(self) -> TrieInputSorted {
        self.input
    }

    pub(crate) const fn skipped_for_reused_sparse_trie(&self) -> bool {
        self.skipped_for_reused_sparse_trie
    }
}

/// Execution state required to initialize an overlay state provider.
///
/// Account entries preserve known non-existence, while storage and code entries contain only data
/// explicitly observed during execution. Accounts never retain database-context-local lookup IDs.
#[derive(Clone, Debug, Default)]
pub struct ExecutionOverlay {
    /// In-memory block hashes in ascending block-number order.
    block_hashes: Vec<BlockNumHash>,
    /// Account state by address, without database-context-local [`AccountInfo::account_id`] hints.
    accounts: AddressMap<Option<AccountInfo>>,
    /// Storage values by address and slot.
    storage: AddressMap<U256Map<U256>>,
    /// Accounts whose storage was wiped during execution.
    ///
    /// An absent slot for one of these accounts is known to be zero, rather than falling back to
    /// the durable database state.
    storage_wipes: AddressSet,
    /// Bytecode by code hash.
    code_hashes: B256Map<Bytecode>,
}

impl ExecutionOverlay {
    /// Returns the in-memory block hashes in ascending block-number order.
    pub const fn block_hashes(&self) -> &[BlockNumHash] {
        self.block_hashes.as_slice()
    }

    /// Returns the account state by address.
    pub const fn accounts(&self) -> &AddressMap<Option<AccountInfo>> {
        &self.accounts
    }

    /// Returns the storage values by address and slot.
    pub const fn storage(&self) -> &AddressMap<U256Map<U256>> {
        &self.storage
    }

    /// Returns an explicitly observed storage value, or zero when the account's storage was
    /// wiped.
    pub(crate) fn storage_value(&self, address: Address, slot: U256) -> Option<U256> {
        self.storage
            .get(&address)
            .and_then(|storage| storage.get(&slot))
            .copied()
            .or_else(|| self.storage_wipes.contains(&address).then_some(U256::ZERO))
    }

    /// Returns the bytecode by code hash.
    pub const fn code_hashes(&self) -> &B256Map<Bytecode> {
        &self.code_hashes
    }

    #[cfg(test)]
    pub(crate) const fn block_hashes_mut(&mut self) -> &mut Vec<BlockNumHash> {
        &mut self.block_hashes
    }

    #[cfg(test)]
    pub(crate) const fn accounts_mut(&mut self) -> &mut AddressMap<Option<AccountInfo>> {
        &mut self.accounts
    }

    #[cfg(test)]
    pub(crate) const fn storage_mut(&mut self) -> &mut AddressMap<U256Map<U256>> {
        &mut self.storage
    }

    #[cfg(test)]
    pub(crate) const fn code_hashes_mut(&mut self) -> &mut B256Map<Bytecode> {
        &mut self.code_hashes
    }

    /// Extends this overlay with the execution state of a later block.
    pub(crate) fn extend_block<N: NodePrimitives>(&mut self, block: &ExecutedBlock<N>) {
        self.block_hashes.push(block.recovered_block().num_hash());
        self.extend_state(&block.execution_output.state);
    }

    /// Extends this overlay with a later bundle state.
    ///
    /// [`AccountInfo::account_id`] is a lookup hint owned by the database context that assigned it
    /// and cannot be reused by the overlay's database context. All other account fields are
    /// preserved.
    fn extend_state(&mut self, state: &BundleState) {
        let (accounts, storage, storage_wipes, code_hashes) =
            (&mut self.accounts, &mut self.storage, &mut self.storage_wipes, &mut self.code_hashes);

        #[allow(unused_mut)]
        let mut extend_accounts_and_storage = || {
            for (address, account) in state.state() {
                accounts.insert(*address, Self::normalized_account_info(account.info.clone()));
                if account.was_destroyed() {
                    storage_wipes.insert(*address);
                    storage.remove(address);
                }
                let account_storage = storage.entry(*address).or_default();
                for (slot, value) in &account.storage {
                    account_storage.insert(*slot, value.present_value);
                }
            }
        };
        #[allow(unused_mut)]
        let mut extend_code_hashes = || {
            code_hashes.extend(state.contracts.iter().map(|(hash, code)| (*hash, code.clone())));
        };

        #[cfg(feature = "rayon")]
        rayon::join(extend_accounts_and_storage, extend_code_hashes);

        #[cfg(not(feature = "rayon"))]
        {
            extend_accounts_and_storage();
            extend_code_hashes();
        }
    }

    #[cfg(test)]
    fn extend_overlay(&mut self, other: &Self) {
        self.block_hashes.extend_from_slice(&other.block_hashes);
        self.accounts.extend(
            other
                .accounts
                .iter()
                .map(|(address, info)| (*address, Self::normalized_account_info(info.clone()))),
        );
        for address in &other.storage_wipes {
            self.storage.remove(address);
        }
        for (address, slots) in &other.storage {
            self.storage
                .entry(*address)
                .or_default()
                .extend(slots.iter().map(|(slot, value)| (*slot, *value)));
        }
        self.storage_wipes.extend(other.storage_wipes.iter().copied());
        self.code_hashes.extend(other.code_hashes.iter().map(|(hash, code)| (*hash, code.clone())));
    }

    /// Removes the database-local account lookup hint before caching account state.
    ///
    /// `account_id` indexes the database or BAL context that produced the [`AccountInfo`]. A later
    /// execution context can assign that ID to a different account, so it must not cross the
    /// execution-overlay boundary.
    const fn normalized_account_info(mut info: Option<AccountInfo>) -> Option<AccountInfo> {
        if let Some(info) = &mut info {
            info.account_id = None;
        }
        info
    }
}

/// Builder for calculating trie and hashed-state overlays.
///
/// This stores the overlay manager, overlay configuration, and the logic for resolving overlays
/// and collecting reverts.
#[derive(Debug, Clone)]
pub struct OverlayBuilder<N: NodePrimitives = EthPrimitives> {
    /// Parent hash requested by the caller.
    parent_hash: B256,
    /// Manager used for cached changesets and overlays.
    overlay_manager: OverlayManager<N>,
    /// Snapshot of the in-memory chain ending at the requested parent.
    parent_state: Option<BlockState<N>>,
    /// Anchor hash of the reused sparse trie, if this task reused one.
    reused_sparse_trie_anchor_hash: Option<B256>,
    /// Whether building the overlay may query revert changesets.
    no_reverts: bool,
    /// Cache behavior for the overlays this builder resolves.
    overlay_cache_config: OverlayCacheConfig,
    /// Metrics for overlay construction.
    metrics: OverlayBuilderMetrics,
}

impl<N: NodePrimitives> OverlayBuilder<N> {
    /// Create a new manager-backed overlay builder.
    pub(crate) fn new(
        parent_hash: B256,
        parent_state: Option<BlockState<N>>,
        overlay_manager: OverlayManager<N>,
    ) -> Self {
        Self {
            parent_hash,
            overlay_manager,
            parent_state,
            reused_sparse_trie_anchor_hash: None,
            no_reverts: false,
            overlay_cache_config: OverlayCacheConfig::default(),
            metrics: OverlayBuilderMetrics::default(),
        }
    }

    /// Skips managed overlay construction when the sparse trie was reused and the DB tip is
    /// already covered by its anchor-to-parent range.
    pub(crate) const fn with_skip_overlay_for_reused_sparse_trie(
        mut self,
        anchor_hash: B256,
    ) -> Self {
        self.reused_sparse_trie_anchor_hash = Some(anchor_hash);
        self
    }

    /// Returns an error instead of querying revert changesets when reverts are required.
    pub(crate) const fn with_no_reverts(mut self) -> Self {
        self.no_reverts = true;
        self
    }

    /// Appends an executed block to this builder's in-memory parent state.
    pub fn with_appended_block(mut self, block: ExecutedBlock<N>) -> Self {
        debug_assert_eq!(block.recovered_block().parent_hash(), self.parent_hash);
        self.parent_hash = block.recovered_block().hash();
        self.parent_state =
            Some(BlockState::with_parent(block, self.parent_state.take().map(Arc::new)));
        self.reused_sparse_trie_anchor_hash = None;
        self.overlay_cache_config.write_to_cache = false;
        self
    }

    /// Returns the durable anchor to use for this builder's parent.
    #[cfg(test)]
    fn anchor_at_parent<Provider>(&self, provider: &Provider) -> ProviderResult<AnchorForParent>
    where
        Provider: StageCheckpointReader + BlockNumReader + PruneCheckpointReader,
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
        Provider: BlockNumReader + PruneCheckpointReader,
    {
        use std::io::Error;

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

        Ok(AnchorForParent::RevertsRequired { anchor, finish })
    }

    /// Builds the effective state trie overlay for the given provider.
    ///
    /// Set `trie_changesets` only for consumers that produce [`TrieUpdates`], such as
    /// [`StateRootProvider::state_root_with_updates`]. Other consumers, including roots, proofs,
    /// multiproofs, and witnesses, should leave it false: reverts are then represented by
    /// hashed-state prefix sets instead of querying trie changesets.
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
            AnchorForParent::RevertsRequired { anchor, .. } => {
                let revert_blocks =
                    self.revert_blocks(&anchor_for_parent)?.expect("reverts are required");

                debug!(
                    target: "storage::overlay",
                    ?revert_blocks,
                    ?anchor,
                    "Collecting trie reverts for overlay state provider"
                );

                let trie_reverts = if trie_changesets {
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
                } else {
                    retrieve_trie_reverts_duration = Duration::ZERO;
                    Arc::default()
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

                let prefix_sets = if trie_changesets {
                    Default::default()
                } else {
                    hashed_state_reverts.construct_prefix_sets()
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

                (trie_updates, hashed_post_state, Default::default())
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
            let parent_state = self.parent_state.as_ref().ok_or_else(|| {
                ProviderError::other(std::io::Error::other(
                    "state trie overlay cannot be anchored without in-memory parent state",
                ))
            })?;
            self.overlay_manager
                .overlay_for_parent(parent_state, anchor_hash, self.overlay_cache_config)
                .map_err(ProviderError::other)
        }
    }

    /// Resolves the execution overlay for the configured in-memory source.
    fn resolve_execution_overlay(
        &self,
        anchor_hash: BlockHash,
    ) -> ProviderResult<Arc<ExecutionOverlay>> {
        if anchor_hash == self.parent_hash {
            Ok(Arc::new(ExecutionOverlay::default()))
        } else {
            let parent_state = self.parent_state.as_ref().ok_or_else(|| {
                ProviderError::other(std::io::Error::other("missing in-memory parent state"))
            })?;
            self.overlay_manager
                .execution_overlay_for_block_state(
                    parent_state,
                    anchor_hash,
                    self.overlay_cache_config,
                )
                .map_err(ProviderError::other)
        }
    }

    /// Returns the blocks to revert from Finish to the selected anchor, if any.
    fn revert_blocks(
        &self,
        anchor_for_parent: &AnchorForParent,
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
    use alloy_primitives::{map::HashMap, Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_db::{
        models::{AccountBeforeTx, BlockNumberAddress},
        tables,
        transaction::DbTxMut,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint};
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{BranchNodeCompact, ComputedTrieData, HashedPostState, HashedStorage, Nibbles};
    use revm::{
        bytecode::Bytecode,
        database::{AccountStatus, BundleAccount, BundleState},
        state::{AccountId, AccountInfo},
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
        let address = Address::with_last_byte(id);
        let slot = U256::from(id);
        let code_hash = B256::with_last_byte(id.saturating_add(64));
        let state = BundleState::builder(block.block_number()..=block.block_number())
            .state_present_account_info(
                address,
                AccountInfo {
                    nonce: id as u64,
                    balance: U256::from(id),
                    account_id: AccountId::new(id as usize),
                    ..Default::default()
                },
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
        overlay.input().state.accounts.iter().map(|(key, _)| *key).collect()
    }

    fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
        overlay.input().nodes.account_nodes_ref().iter().map(|(path, _)| *path).collect()
    }

    #[test]
    fn execution_overlay_extends_bundle_state_without_account_ids() {
        let address = Address::with_last_byte(1);
        let slot = U256::from(2);
        let value = U256::from(3);
        let code = Bytecode::new_raw(vec![0x60, 0x00].into());
        let code_hash = code.hash_slow();
        let account = AccountInfo {
            nonce: 4,
            balance: U256::from(5),
            code_hash,
            code: Some(code.clone()),
            account_id: AccountId::new(6),
        };
        let state = BundleState::builder(0..=0)
            .state_present_account_info(address, account.clone())
            .state_storage(address, HashMap::from_iter([(slot, (U256::ZERO, value))]))
            .contract(code_hash, code.clone())
            .build();
        assert!(state.state()[&address].info.as_ref().unwrap().account_id.is_some());

        let mut overlay = ExecutionOverlay::default();
        overlay.extend_state(&state);

        let stored_account = overlay.accounts[&address].as_ref().unwrap();
        assert_eq!(stored_account.account_id, None);
        assert_eq!(
            stored_account,
            &AccountInfo { account_id: None, ..account },
            "normalization must preserve durable account fields"
        );
        assert_eq!(stored_account.code, Some(code.clone()));
        assert_eq!(overlay.storage[&address][&slot], value);
        assert_eq!(overlay.code_hashes[&code_hash], code);
    }

    #[test]
    fn execution_overlay_zeroes_unobserved_storage_for_destroyed_accounts() {
        let address = Address::with_last_byte(1);
        let mut state = BundleState::default();
        state.state.insert(
            address,
            BundleAccount::new(
                Some(AccountInfo::default()),
                None,
                Default::default(),
                AccountStatus::Destroyed,
            ),
        );

        let mut overlay = ExecutionOverlay::default();
        overlay.extend_state(&state);

        assert_eq!(overlay.storage_value(address, U256::ZERO), Some(U256::ZERO));
    }

    #[test]
    fn execution_overlay_composition_uses_later_values_and_normalizes_accounts() {
        let address = Address::with_last_byte(1);
        let retained_address = Address::with_last_byte(2);
        let slot = U256::from(3);
        let retained_slot = U256::from(4);
        let first_code_hash = B256::with_last_byte(5);
        let later_code_hash = B256::with_last_byte(6);
        let first_block = BlockNumHash::new(1, B256::with_last_byte(7));
        let later_block = BlockNumHash::new(2, B256::with_last_byte(8));

        let mut overlay = ExecutionOverlay::default();
        overlay.block_hashes.push(first_block);
        overlay.accounts.insert(
            address,
            Some(AccountInfo { nonce: 1, account_id: None, ..Default::default() }),
        );
        overlay.accounts.insert(retained_address, Some(AccountInfo::default()));
        overlay.storage.entry(address).or_default().insert(slot, U256::from(9));
        overlay.storage.entry(address).or_default().insert(retained_slot, U256::from(10));
        overlay.code_hashes.insert(first_code_hash, Bytecode::new_raw(vec![1].into()));

        let mut later = ExecutionOverlay::default();
        later.block_hashes.push(later_block);
        later.accounts.insert(
            address,
            Some(AccountInfo { nonce: 11, account_id: AccountId::new(12), ..Default::default() }),
        );
        later.storage.entry(address).or_default().insert(slot, U256::from(13));
        later.storage_wipes.insert(address);
        later.code_hashes.insert(later_code_hash, Bytecode::new_raw(vec![2].into()));

        overlay.extend_overlay(&later);

        assert!(later.accounts[&address].as_ref().unwrap().account_id.is_some());
        assert_eq!(overlay.block_hashes, vec![first_block, later_block]);
        assert_eq!(overlay.accounts[&address].as_ref().unwrap().nonce, 11);
        assert_eq!(overlay.accounts[&address].as_ref().unwrap().account_id, None);
        assert!(overlay.accounts.contains_key(&retained_address));
        assert_eq!(overlay.storage[&address][&slot], U256::from(13));
        assert!(!overlay.storage[&address].contains_key(&retained_slot));
        assert_eq!(overlay.storage_value(address, U256::from(14)), Some(U256::ZERO));
        assert!(overlay.code_hashes.contains_key(&first_code_hash));
        assert!(overlay.code_hashes.contains_key(&later_code_hash));
    }

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
                .build_state_trie_overlay(&provider, true)
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

    #[test]
    fn managed_overlay_skips_when_finish_is_the_anchor() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[4].clone());
        let provider = factory.provider().unwrap();

        let overlay = manager
            .overlay_builder(blocks[4].recovered_block().hash())
            .with_skip_overlay_for_reused_sparse_trie(blocks[3].recovered_block().hash())
            .build_state_trie_overlay(&provider, true)
            .unwrap();

        assert!(overlay.input().state.is_empty());
        assert!(overlay.input().nodes.is_empty());
    }

    #[test]
    fn no_reverts_errors_when_reverts_are_required() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let provider = factory.provider().unwrap();

        let builder = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[1].recovered_block().hash())
            .with_no_reverts();
        let error = builder.build_state_trie_overlay(&provider, true).unwrap_err();

        assert!(error.to_string().contains("reverts are disabled"));
    }

    #[test]
    fn appended_overlay_rejects_noncanonical_anchor_when_reverts_are_required() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let parent_hash = B256::with_last_byte(100);
        assert_ne!(parent_hash, blocks[1].recovered_block().hash());
        let block = TestBlockBuilder::eth()
            .get_executed_block_with_number(blocks[2].block_number(), parent_hash);
        let builder =
            OverlayManager::default().overlay_builder(parent_hash).with_appended_block(block);

        assert!(matches!(
            builder.execution_overlay(&provider),
            Err(ProviderError::BlockHashNotFound(hash)) if hash == parent_hash
        ));
        assert!(matches!(
            builder.build_state_trie_overlay(&provider, true),
            Err(ProviderError::BlockHashNotFound(hash)) if hash == parent_hash
        ));
    }

    #[test]
    fn state_trie_overlay_uses_revert_prefix_sets_without_trie_changesets() {
        let (factory, blocks) = setup_frontiers(2, 3);
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

        let provider = factory.provider().unwrap();
        let overlay = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[1].recovered_block().hash())
            .build_state_trie_overlay(&provider, false)
            .unwrap();

        assert!(overlay.input().nodes.is_empty());
        assert!(!overlay.input().prefix_sets.is_empty());
    }

    #[test]
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

        let provider = factory.provider().unwrap();
        let (overlay, fallback_block_number) = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[1].recovered_block().hash())
            .execution_overlay(&provider)
            .unwrap();

        assert_eq!(fallback_block_number, Some(2));
        assert!(overlay.accounts.is_empty());
        assert!(overlay.storage.is_empty());
        assert!(overlay.code_hashes.is_empty());
    }

    #[test]
    fn execution_overlay_uses_managed_blocks_after_the_anchor() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=4] {
            manager.insert_block(block.clone());
        }
        let provider = factory.provider().unwrap();

        let (overlay, fallback_block_number) = manager
            .overlay_builder(blocks[3].recovered_block().hash())
            .execution_overlay(&provider)
            .unwrap();

        assert_eq!(fallback_block_number, None);

        for id in [3, 4] {
            let address = Address::with_last_byte(id);
            let slot = U256::from(id);
            assert_eq!(overlay.accounts[&address].as_ref().unwrap().balance, U256::from(id));
            assert_eq!(overlay.accounts[&address].as_ref().unwrap().account_id, None);
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

    #[test]
    fn execution_overlay_marks_historical_fallback_for_managed_fork() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let address = Address::with_last_byte(1);
        let slot = U256::from(1);
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
            "the managed chain must not contain the durable Finish block"
        );

        let manager = OverlayManager::default();
        manager.insert_block(side_block_two.clone());
        manager.insert_block(side_block_three.clone());
        let provider = factory.provider().unwrap();

        let (overlay, fallback_block_number) = manager
            .overlay_builder(side_block_three.recovered_block().hash())
            .execution_overlay(&provider)
            .unwrap();

        assert_eq!(fallback_block_number, Some(2));

        assert_eq!(overlay.accounts[&address].as_ref().unwrap().balance, U256::from(1));
        assert_eq!(overlay.accounts[&address].as_ref().unwrap().account_id, None);
        assert_eq!(overlay.storage[&address][&slot], U256::from(1));
        assert_eq!(
            overlay.block_hashes,
            [side_block_two, side_block_three]
                .iter()
                .map(|block| block.recovered_block().num_hash())
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn execution_overlay_no_revert_path_discards_account_ids() {
        let (factory, blocks) = setup_frontiers(1, 1);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let provider = factory.provider().unwrap();

        let (overlay, fallback_block_number) = manager
            .overlay_builder(blocks[3].recovered_block().hash())
            .execution_overlay(&provider)
            .unwrap();

        assert_eq!(fallback_block_number, None);
        assert_eq!(overlay.accounts.len(), 2);
        assert!(overlay.accounts.values().flatten().all(|account| account.account_id.is_none()));
    }

    #[test]
    fn managed_overlay_uses_persisted_parent_even_if_retained() {
        let (factory, blocks) = setup_frontiers(2, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[1].clone());
        let provider = factory.provider().unwrap();
        let builder = manager.overlay_builder(blocks[1].recovered_block().hash());
        match builder.anchor_at_parent(&provider).unwrap() {
            AnchorForParent::RevertsRequired { anchor, finish } => {
                assert_eq!(anchor, blocks[1].recovered_block().num_hash());
                assert_eq!(finish, blocks[3].recovered_block().num_hash());
            }
            AnchorForParent::NoReverts { .. } => {
                panic!("persisted parent below Finish must require reverts")
            }
        }
    }

    #[test]
    fn overlay_after_state_trie_frontier_requires_managed_coverage() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let error = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(blocks[3].recovered_block().hash())
            .build_state_trie_overlay(&provider, true)
            .unwrap_err();

        assert!(
            error.to_string().contains("is after partial state trie frontier"),
            "unexpected error: {error}"
        );
    }

    #[test]
    fn managed_overlay_errors_if_parent_is_not_persisted_or_managed_across_frontiers() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let provider = factory.provider().unwrap();
        let parent_hash = blocks[3].recovered_block().hash();
        let error = OverlayManager::<EthPrimitives>::default()
            .overlay_builder(parent_hash)
            .build_state_trie_overlay(&provider, true)
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
