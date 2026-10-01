use crate::{database_state_frontiers, ExecutionOverlay, OverlayBuilder, StateTrieOverlay};
use alloy_primitives::{Address, BlockHash, BlockNumber, B256, U256};
use metrics::{Counter, Histogram};
use reth_db_api::{cursor::DbDupCursorRO, tables, transaction::DbTx, DatabaseError};
use reth_errors::{ProviderError, ProviderResult};
use reth_ethereum_primitives::EthPrimitives;
use reth_metrics::Metrics;
use reth_primitives_traits::{
    dashmap::{self, DashMap},
    Account, NodePrimitives,
};
use reth_prune_types::PruneSegment;
use reth_storage_api::{
    AccountReader, BlockHashReader, BlockNumReader, BytecodeReader, ChangeSetReader, DBProvider,
    DatabaseProviderFactory, DatabaseProviderROFactory, DbTxProvider, HashedPostStateProvider,
    HistoryInfo, HistoryReader, PruneCheckpointReader, StageCheckpointReader, StateProofProvider,
    StateProvider, StateRootProvider, StorageChangeSetReader, StorageRootProvider,
    StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{
        zero_destroyed_account_storage, HashedCursorFactory, HashedPostStateCursor,
        HashedPostStateCursorFactory,
    },
    proof::{Proof, StorageProof as TrieStorageProof},
    state_trie_cursor::{InMemoryStateTrieCursor, StateTrieCursor, StateTrieCursorFactory},
    trie_cursor::{InMemoryTrieCursor, InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, DecodedMultiProofV2, ExecutionWitnessMode, HashedPostState,
    HashedPostStateSorted, HashedStorage, KeccakKeyHasher, MultiProof, MultiProofTargets,
    MultiProofTargetsV2, Nibbles, StateRoot, StateTrieNode, StateTrieUpdatesSorted,
    StorageMultiProof, StorageProof, StorageRoot, TrieInput, TrieInputSorted,
};
use std::{cell::OnceCell, fmt, ops::Deref, sync::Arc, time::Instant};
use tracing::instrument;

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
    /// A cache mapping `(state_trie_tip, finish_tip, trie_changesets)` to [`StateTrieOverlay`].
    ///
    /// Under partial persistence the overlay depends on both durable frontiers, so both hashes are
    /// part of the cache key.
    state_trie_overlay_cache: StateTrieOverlayCache,
    /// Metrics for provider factory operations.
    metrics: OverlayStateProviderFactoryMetrics,
}

impl<F, N: NodePrimitives> OverlayStateProviderFactory<F, N> {
    /// Create a new overlay state provider factory.
    pub fn new(factory: F, overlay_builder: OverlayBuilder<N>) -> Self {
        Self {
            factory,
            overlay_builder,
            state_trie_overlay_cache: Default::default(),
            metrics: Default::default(),
        }
    }

    /// Skips managed overlay construction when this factory is used by a task that reused a sparse
    /// trie covering both durable frontiers through the parent.
    pub fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.overlay_builder =
            self.overlay_builder.with_skip_overlay_for_reused_sparse_trie(anchor_hash);
        self.state_trie_overlay_cache = Default::default();
        self
    }
}

impl<F, N> DatabaseProviderROFactory for OverlayStateProviderFactory<F, N>
where
    N: NodePrimitives,
    F: DatabaseProviderFactory,
    F::Provider: StageCheckpointReader
        + PruneCheckpointReader
        + BlockNumReader
        + ChangeSetReader
        + StorageChangeSetReader
        + StorageSettingsCache,
{
    type Provider = OverlayStateProvider<OwnedProvider<F::Provider>, N>;

    /// Create a read-only [`OverlayStateProvider`].
    #[instrument(level = "debug", target = "providers::state::overlay", skip_all)]
    fn database_provider_ro(
        &self,
    ) -> ProviderResult<OverlayStateProvider<OwnedProvider<F::Provider>, N>> {
        let overall_start = Instant::now();

        // Get a read-only provider
        let provider = {
            let start = Instant::now();
            let res = self.factory.database_provider_ro()?;
            self.metrics.create_provider_duration.record(start.elapsed());
            res
        };

        let is_v2 = provider.cached_storage_settings().is_v2();
        self.metrics.database_provider_ro_duration.record(overall_start.elapsed());
        Ok(OverlayStateProvider::new_with_caches(
            provider,
            self.overlay_builder.clone(),
            Arc::clone(&self.state_trie_overlay_cache),
            self.metrics.clone(),
            is_v2,
        ))
    }
}

/// State provider with lazily resolved state trie and execution overlays.
pub struct OverlayStateProvider<Provider, N: NodePrimitives = EthPrimitives> {
    provider: Provider,
    overlay_builder: Option<OverlayBuilder<N>>,
    state_trie_overlay_cache: StateTrieOverlayCache,
    metrics: OverlayStateProviderFactoryMetrics,
    state_trie_overlay: OnceCell<StateTrieOverlay>,
    state_trie_overlay_with_trie_changesets: OnceCell<StateTrieOverlay>,
    execution_overlay: OnceCell<CachedExecutionOverlay>,
    is_v2: bool,
}

impl<Provider, N: NodePrimitives> OverlayStateProvider<OwnedProvider<Provider>, N> {
    /// Creates an overlay state provider over an already-open database provider.
    pub fn new(provider: Provider, overlay_builder: OverlayBuilder<N>) -> Self
    where
        Provider: StorageSettingsCache,
    {
        let is_v2 = provider.cached_storage_settings().is_v2();
        Self::new_with_caches(
            provider,
            overlay_builder,
            Default::default(),
            Default::default(),
            is_v2,
        )
    }

    const fn new_with_caches(
        provider: Provider,
        overlay_builder: OverlayBuilder<N>,
        state_trie_overlay_cache: StateTrieOverlayCache,
        metrics: OverlayStateProviderFactoryMetrics,
        is_v2: bool,
    ) -> Self {
        Self {
            provider: OwnedProvider(provider),
            overlay_builder: Some(overlay_builder),
            state_trie_overlay_cache,
            metrics,
            state_trie_overlay: OnceCell::new(),
            state_trie_overlay_with_trie_changesets: OnceCell::new(),
            execution_overlay: OnceCell::new(),
            is_v2,
        }
    }

    #[cfg(test)]
    fn new_with_execution(
        provider: Provider,
        execution_overlay: Arc<ExecutionOverlay>,
        is_v2: bool,
    ) -> Self {
        Self {
            provider: OwnedProvider(provider),
            overlay_builder: None,
            state_trie_overlay_cache: Default::default(),
            metrics: Default::default(),
            state_trie_overlay: OnceCell::new(),
            state_trie_overlay_with_trie_changesets: OnceCell::new(),
            execution_overlay: OnceCell::from(CachedExecutionOverlay {
                overlay: execution_overlay,
                historical_fallback: None,
            }),
            is_v2,
        }
    }
}

impl<'a, Provider, N: NodePrimitives> OverlayStateProvider<&'a Provider, N> {
    /// Creates an overlay state provider over a borrowed database provider.
    pub fn new_ref(provider: &'a Provider, overlay_builder: OverlayBuilder<N>) -> Self
    where
        Provider: StorageSettingsCache,
    {
        let is_v2 = provider.cached_storage_settings().is_v2();
        Self {
            provider,
            overlay_builder: Some(overlay_builder),
            state_trie_overlay_cache: Default::default(),
            metrics: Default::default(),
            state_trie_overlay: OnceCell::new(),
            state_trie_overlay_with_trie_changesets: OnceCell::new(),
            execution_overlay: OnceCell::new(),
            is_v2,
        }
    }

    pub(crate) fn new_with_state_trie(
        provider: &'a Provider,
        state_trie_overlay: StateTrieOverlay,
        is_v2: bool,
    ) -> Self {
        Self {
            provider,
            overlay_builder: None,
            state_trie_overlay_cache: Default::default(),
            metrics: Default::default(),
            state_trie_overlay: OnceCell::from(state_trie_overlay.clone()),
            state_trie_overlay_with_trie_changesets: OnceCell::from(state_trie_overlay),
            execution_overlay: OnceCell::new(),
            is_v2,
        }
    }
}

impl<Provider, N: NodePrimitives> OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: Sized,
{
    fn provider(&self) -> &Provider::Target {
        &self.provider
    }

    fn state_trie_overlay(&self, trie_changesets: bool) -> ProviderResult<&StateTrieOverlay>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + HashedCursorFactory
            + TrieCursorFactory
            + BlockNumReader
            + StorageSettingsCache,
    {
        let state_trie_overlay = self.state_trie_overlay_mut(trie_changesets);
        if let Some(overlay) = state_trie_overlay.get() {
            return Ok(overlay)
        }

        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(self.provider())?;
        let overlay = match self.state_trie_overlay_cache.entry((
            state_trie_tip_block.hash,
            finish_tip_block.hash,
            trie_changesets,
        )) {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                self.metrics.state_trie_overlay_cache_misses.increment(1);
                let overlay = self
                    .overlay_builder
                    .as_ref()
                    .expect("state trie overlay must be initialized or lazily resolvable")
                    .build_state_trie_overlay_at_frontiers(
                        self.provider(),
                        state_trie_tip_block,
                        finish_tip_block,
                        trie_changesets,
                    )?;
                if !overlay.skipped_for_reused_sparse_trie() {
                    entry.insert(overlay.clone());
                }
                overlay
            }
        };
        let _ = state_trie_overlay.set(overlay);
        Ok(state_trie_overlay.get().expect("state trie overlay was just initialized"))
    }

    const fn state_trie_overlay_mut(&self, trie_changesets: bool) -> &OnceCell<StateTrieOverlay> {
        if trie_changesets {
            &self.state_trie_overlay_with_trie_changesets
        } else {
            &self.state_trie_overlay
        }
    }

    fn build_overlay(
        &self,
        input: TrieInputSorted,
        trie_changesets: bool,
    ) -> ProviderResult<TrieInputSorted>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + HashedCursorFactory
            + TrieCursorFactory
            + BlockNumReader
            + StorageSettingsCache,
    {
        let overlay = self.state_trie_overlay(trie_changesets)?;
        if overlay.skipped_for_reused_sparse_trie() {
            return Err(ProviderError::UnsupportedProvider)
        }
        let TrieInputSorted {
            nodes: input_nodes,
            state: input_state,
            mut prefix_sets,
            state_trie: input_state_trie,
        } = input;
        let overlay_input = overlay.input();
        let mut nodes = Arc::clone(&overlay_input.nodes);
        let mut state = Arc::clone(&overlay_input.state);

        if !input_nodes.is_empty() {
            Arc::make_mut(&mut nodes).extend_ref_and_sort(&input_nodes);
        }
        if !input_state.is_empty() {
            Arc::make_mut(&mut state).extend_ref_and_sort(&input_state);
        }

        prefix_sets.extend_ref(&overlay_input.prefix_sets);
        let mut input = TrieInputSorted::new(nodes, state, prefix_sets);
        input.state_trie = Arc::new(StateTrieUpdatesSorted::merge_slice(&[
            input_state_trie.as_ref(),
            overlay_input.state_trie.as_ref(),
        ]));
        Ok(input)
    }

    fn execution_overlay(
        &self,
    ) -> ProviderResult<(&Arc<ExecutionOverlay>, Option<&HistoricalFallback>)>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + HashedCursorFactory
            + TrieCursorFactory
            + BlockNumReader,
    {
        if let Some(overlay) = self.execution_overlay.get() {
            return Ok((&overlay.overlay, overlay.historical_fallback.as_ref()))
        }

        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(self.provider())?;
        let (overlay, fallback_block_number) = self
            .overlay_builder
            .as_ref()
            .expect("execution overlay must be initialized or lazily resolvable")
            .execution_overlay_at_frontiers(
                self.provider(),
                state_trie_tip_block,
                finish_tip_block,
            )?;
        let historical_fallback = fallback_block_number
            .map(|block_number| {
                let account_history_block_number = self
                    .provider()
                    .get_prune_checkpoint(PruneSegment::AccountHistory)?
                    .and_then(|checkpoint| checkpoint.block_number)
                    .map(|block_number| block_number + 1);
                if account_history_block_number.is_some_and(|lowest| block_number < lowest) {
                    return Err(ProviderError::StateAtBlockPruned(block_number))
                }

                let storage_history_block_number = self
                    .provider()
                    .get_prune_checkpoint(PruneSegment::StorageHistory)?
                    .and_then(|checkpoint| checkpoint.block_number)
                    .map(|block_number| block_number + 1);
                if storage_history_block_number.is_some_and(|lowest| block_number < lowest) {
                    return Err(ProviderError::StateAtBlockPruned(block_number))
                }

                Ok(HistoricalFallback {
                    block_number,
                    account_history_block_number,
                    storage_history_block_number,
                })
            })
            .transpose()?;
        let overlay = CachedExecutionOverlay { overlay, historical_fallback };
        let _ = self.execution_overlay.set(overlay);
        let overlay = self.execution_overlay.get().expect("execution overlay was just initialized");
        Ok((&overlay.overlay, overlay.historical_fallback.as_ref()))
    }
}

impl<Provider, N: NodePrimitives> fmt::Debug for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: fmt::Debug + Sized,
{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("OverlayStateProvider")
            .field("provider", self.provider())
            .field("state_trie_overlay", &self.state_trie_overlay.get())
            .field("execution_overlay", &self.execution_overlay.get())
            .field("is_v2", &self.is_v2)
            .finish()
    }
}

impl<Provider, N: NodePrimitives> AccountReader for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StateTrieCursorFactory
        + HistoryReader
        + StorageSettingsCache
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let (overlay, historical_fallback) = self.execution_overlay()?;
        if let Some(account) = overlay.accounts().get(address) {
            return Ok(account.as_ref().map(Account::from))
        }
        if let Some(historical_fallback) = historical_fallback {
            if cfg!(any(feature = "state-trie-db", feature = "legacy-trie-rocksdb")) {
                return Err(ProviderError::UnsupportedProvider)
            }
            return match self.provider().account_history_info(
                *address,
                historical_fallback.block_number,
                historical_fallback.account_history_block_number,
            )? {
                HistoryInfo::NotYetWritten => Ok(None),
                HistoryInfo::InChangeset(changeset_block_number) => self
                    .provider()
                    .get_account_before_block(changeset_block_number, *address)?
                    .ok_or(ProviderError::AccountChangesetNotFound {
                        block_number: changeset_block_number,
                        address: *address,
                    })
                    .map(|account_before| account_before.info),
                HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => {
                    self.basic_account_from_db(address)
                }
            }
        }
        self.basic_account_from_db(address)
    }
}

impl<Provider, N: NodePrimitives> OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StateTrieCursorFactory
        + StorageSettingsCache,
{
    fn basic_account_from_db(&self, address: &Address) -> ProviderResult<Option<Account>> {
        if cfg!(feature = "state-trie-db") {
            let path = Nibbles::unpack(alloy_primitives::keccak256(address));
            Ok(self.provider().state_trie_account_cursor()?.get(path)?.and_then(
                |node| match node {
                    StateTrieNode::Leaf { value, .. } => Some(value.into()),
                    _ => None,
                },
            ))
        } else if cfg!(feature = "legacy-trie-rocksdb") {
            self.provider().hashed_account(alloy_primitives::keccak256(address)).map_err(Into::into)
        } else if self.provider().cached_storage_settings().use_hashed_state() {
            let hashed_address = alloy_primitives::keccak256(address);
            self.provider()
                .tx()
                .get_by_encoded_key::<tables::HashedAccounts>(&hashed_address)
                .map_err(Into::into)
        } else {
            self.provider()
                .tx()
                .get_by_encoded_key::<tables::PlainAccountState>(address)
                .map_err(Into::into)
        }
    }
}

impl<Provider, N: NodePrimitives> BlockHashReader for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: BlockHashReader
        + DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + Sized
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        let (overlay, _) = self.execution_overlay()?;
        if let Some(block) = overlay.block_hashes().iter().find(|block| block.number == number) {
            return Ok(Some(block.hash))
        }
        self.provider().block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        let (overlay, _) = self.execution_overlay()?;
        let mut block_hashes =
            overlay.block_hashes().iter().filter(|block| (start..end).contains(&block.number));
        let Some(first_block) = block_hashes.next() else {
            return self.provider().canonical_hashes_range(start, end)
        };

        let mut hashes = self.provider().canonical_hashes_range(start, first_block.number)?;
        hashes.push(first_block.hash);
        hashes.extend(block_hashes.map(|block| block.hash));
        Ok(hashes)
    }
}

impl<Provider, N: NodePrimitives> BytecodeReader for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        let (overlay, _) = self.execution_overlay()?;
        if let Some(bytecode) = overlay.code_hashes().get(code_hash) {
            return Ok(Some(reth_primitives_traits::Bytecode(bytecode.clone())));
        }
        self.provider().tx().get_by_encoded_key::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

impl<Provider, N: NodePrimitives> StateRootProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let input =
            self.build_overlay(TrieInputSorted::from_state(hashed_state.into_sorted()), false)?;
        Ok(StateRoot::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()?)
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        let input = self.build_overlay(TrieInputSorted::from_unsorted(input), false)?;
        Ok(StateRoot::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root()?)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let input =
            self.build_overlay(TrieInputSorted::from_state(hashed_state.into_sorted()), true)?;
        Ok(StateRoot::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()?)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let input = self.build_overlay(TrieInputSorted::from_unsorted(input), true)?;
        Ok(StateRoot::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets(input.prefix_sets.freeze())
        .root_with_updates()?)
    }
}

impl<Provider, N: NodePrimitives> StorageRootProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        let hashed_address = alloy_primitives::keccak256(address);
        let mut input = self.build_overlay(
            TrieInputSorted::from_state(
                HashedPostState::from_hashed_storage(hashed_address, hashed_storage).into_sorted(),
            ),
            false,
        )?;
        let prefixes =
            input.prefix_sets.storage_prefix_sets.remove(&hashed_address).unwrap_or_default();
        StorageRoot::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
            address,
            prefixes.freeze(),
            reth_trie::metrics::TrieRootMetrics::new(reth_trie::TrieType::Storage),
        )
        .root()
        .map_err(|err| ProviderError::Database(err.into()))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        self.storage_multiproof(address, &[slot], hashed_storage)?
            .storage_proof(slot)
            .map_err(ProviderError::from)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        let hashed_address = alloy_primitives::keccak256(address);
        let mut input = self.build_overlay(
            TrieInputSorted::from_state(
                HashedPostState::from_hashed_storage(hashed_address, hashed_storage).into_sorted(),
            ),
            false,
        )?;
        let prefixes =
            input.prefix_sets.storage_prefix_sets.remove(&hashed_address).unwrap_or_default();
        TrieStorageProof::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
            address,
        )
        .with_prefix_set_mut(prefixes)
        .storage_multiproof(slots.iter().map(alloy_primitives::keccak256).collect())
        .map_err(ProviderError::from)
    }
}

impl<Provider, N: NodePrimitives> StateProofProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    fn proof(
        &self,
        input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        let input = self.build_overlay(TrieInputSorted::from_unsorted(input), false)?;
        Proof::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .account_proof(address, slots)
        .map_err(ProviderError::from)
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        let input = self.build_overlay(TrieInputSorted::from_unsorted(input), false)?;
        Proof::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .multiproof(targets)
        .map_err(ProviderError::from)
    }

    fn multiproof_v2(
        &self,
        input: TrieInput,
        targets: MultiProofTargetsV2,
    ) -> ProviderResult<DecodedMultiProofV2> {
        let input = self.build_overlay(TrieInputSorted::from_unsorted(input), false)?;
        Proof::new(
            InMemoryTrieCursorFactory::new(self.provider(), input.nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), input.state.as_ref()),
        )
        .with_prefix_sets_mut(input.prefix_sets)
        .multiproof_v2(targets)
        .map_err(ProviderError::from)
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        let TrieInputSorted { nodes, state, prefix_sets, .. } =
            self.build_overlay(TrieInputSorted::from_unsorted(input), false)?;
        let witness = TrieWitness::new(
            InMemoryTrieCursorFactory::new(self.provider(), nodes.as_ref()),
            HashedPostStateCursorFactory::new(self.provider(), state.as_ref()),
        )
        .with_prefix_sets_mut(prefix_sets)
        .with_execution_witness_mode(mode);
        let witness =
            if mode.is_canonical() { witness } else { witness.always_include_root_node() };
        let mut values: Vec<_> = witness.compute(target)?.into_values().collect();
        if mode.is_canonical() {
            values.sort_unstable();
        }
        Ok(values)
    }
}

impl<Provider, N: NodePrimitives> HashedPostStateProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    fn hashed_post_state(
        &self,
        bundle_state: &revm::database::BundleState,
    ) -> ProviderResult<HashedPostState> {
        let mut hashed_state =
            HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state());
        if !bundle_state
            .state()
            .values()
            .any(|account| account.was_destroyed() && account.original_info.is_some())
        {
            return Ok(hashed_state)
        }

        let overlay_state = self.build_overlay(TrieInputSorted::default(), false)?.state;
        zero_destroyed_account_storage(
            &HashedPostStateCursorFactory::new(self.provider(), overlay_state.as_ref()),
            bundle_state.state(),
            &mut hashed_state,
        )?;
        Ok(hashed_state)
    }
}

impl<Provider, N: NodePrimitives> StateProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StateTrieCursorFactory
        + HistoryReader
        + BlockHashReader
        + StorageSettingsCache
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn storage(
        &self,
        address: Address,
        storage_key: alloy_primitives::StorageKey,
    ) -> ProviderResult<Option<alloy_primitives::StorageValue>> {
        let (overlay, historical_fallback) = self.execution_overlay()?;
        if let Some(value) = overlay.storage_value(address, U256::from_be_bytes(storage_key.0)) {
            return Ok(Some(value));
        }
        if let Some(historical_fallback) = historical_fallback {
            if cfg!(any(feature = "state-trie-db", feature = "legacy-trie-rocksdb")) {
                return Err(ProviderError::UnsupportedProvider)
            }
            return match self.provider().storage_history_info(
                address,
                storage_key,
                historical_fallback.block_number,
                historical_fallback.storage_history_block_number,
            )? {
                HistoryInfo::NotYetWritten => Ok(None),
                HistoryInfo::InChangeset(changeset_block_number) => self
                    .provider()
                    .get_storage_before_block(changeset_block_number, address, storage_key)?
                    .ok_or_else(|| ProviderError::StorageChangesetNotFound {
                        block_number: changeset_block_number,
                        address,
                        storage_key: Box::new(storage_key),
                    })
                    .map(|entry| Some(entry.value)),
                HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => {
                    self.storage_from_db(address, storage_key, true)
                }
            }
        }
        self.storage_from_db(address, storage_key, false)
    }

    fn storage_batch(
        &self,
        address: Address,
        storage_keys: &[alloy_primitives::StorageKey],
    ) -> ProviderResult<Vec<Option<alloy_primitives::StorageValue>>> {
        if !cfg!(any(feature = "state-trie-db", feature = "legacy-trie-rocksdb")) {
            return storage_keys.iter().map(|key| self.storage(address, *key)).collect()
        }
        let (overlay, historical_fallback) = self.execution_overlay()?;
        let mut missing = Vec::new();
        let mut paths = Vec::new();
        let mut hashed_slots = Vec::new();
        let mut values: Vec<_> = storage_keys
            .iter()
            .enumerate()
            .map(|(index, key)| {
                let value = overlay.storage_value(address, U256::from_be_bytes(key.0));
                if value.is_none() {
                    missing.push(index);
                    let hash = alloy_primitives::keccak256(key);
                    if cfg!(feature = "legacy-trie-rocksdb") && !cfg!(feature = "state-trie-db") {
                        hashed_slots.push(hash);
                    } else {
                        paths.push(Nibbles::unpack(hash));
                    }
                }
                value
            })
            .collect();
        if !missing.is_empty() {
            if historical_fallback.is_some() {
                return Err(ProviderError::UnsupportedProvider)
            }
            if cfg!(feature = "legacy-trie-rocksdb") && !cfg!(feature = "state-trie-db") {
                let stored = self
                    .provider()
                    .hashed_storage_batch(alloy_primitives::keccak256(address), &hashed_slots)?;
                for (index, value) in missing.into_iter().zip(stored) {
                    values[index] = value;
                }
                return Ok(values)
            }
            let nodes = self
                .provider()
                .state_trie_storage_cursor(alloy_primitives::keccak256(address))?
                .get_batch(&paths)?;
            for (index, node) in missing.into_iter().zip(nodes) {
                values[index] = node.and_then(|node| match node {
                    StateTrieNode::Leaf { value, .. } => Some(value),
                    _ => None,
                });
            }
        }
        Ok(values)
    }
}

impl<Provider, N: NodePrimitives> OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StateTrieCursorFactory
        + StorageSettingsCache,
{
    fn storage_from_db(
        &self,
        address: Address,
        storage_key: alloy_primitives::StorageKey,
        zero_if_missing: bool,
    ) -> ProviderResult<Option<alloy_primitives::StorageValue>> {
        if cfg!(feature = "state-trie-db") {
            let path = Nibbles::unpack(alloy_primitives::keccak256(storage_key));
            let value = self
                .provider()
                .state_trie_storage_cursor(alloy_primitives::keccak256(address))?
                .get(path)?
                .and_then(|node| match node {
                    StateTrieNode::Leaf { value, .. } => Some(value),
                    _ => None,
                });
            Ok(value.or_else(|| zero_if_missing.then_some(U256::ZERO)))
        } else if cfg!(feature = "legacy-trie-rocksdb") {
            let value = self.provider().hashed_storage(
                alloy_primitives::keccak256(address),
                alloy_primitives::keccak256(storage_key),
            )?;
            Ok(value.or_else(|| zero_if_missing.then_some(U256::ZERO)))
        } else if self.provider().cached_storage_settings().use_hashed_state() {
            let hashed_address = alloy_primitives::keccak256(address);
            let hashed_slot = alloy_primitives::keccak256(storage_key);
            let mut cursor = self.provider().tx().cursor_dup_read::<tables::HashedStorages>()?;
            let value = cursor
                .seek_by_key_subkey(hashed_address, hashed_slot)?
                .filter(|entry| entry.key == hashed_slot)
                .map(|entry| entry.value);
            Ok(value.or_else(|| zero_if_missing.then_some(U256::ZERO)))
        } else {
            let mut cursor = self.provider().tx().cursor_dup_read::<tables::PlainStorageState>()?;
            if let Some(entry) = cursor.seek_by_key_subkey(address, storage_key)? &&
                entry.key == storage_key
            {
                return Ok(Some(entry.value))
            }
            Ok(zero_if_missing.then_some(U256::ZERO))
        }
    }
}

impl<Provider, N: NodePrimitives> TrieCursorFactory for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    type AccountTrieCursor<'a>
        = InMemoryTrieCursor<'a, <Provider::Target as TrieCursorFactory>::AccountTrieCursor<'a>>
    where
        Self: 'a;

    type StorageTrieCursor<'a>
        = InMemoryTrieCursor<'a, <Provider::Target as TrieCursorFactory>::StorageTrieCursor<'a>>
    where
        Self: 'a;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        let cursor = self.provider().account_trie_cursor()?;
        Ok(InMemoryTrieCursor::new_account(cursor, &overlay.input().nodes))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        let cursor = self.provider().storage_trie_cursor(hashed_address)?;
        Ok(InMemoryTrieCursor::new_storage(cursor, &overlay.input().nodes, hashed_address))
    }
}

impl<Provider, N: NodePrimitives> HashedCursorFactory for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    type AccountCursor<'a>
        = <HashedPostStateCursorFactory<
        &'a Provider::Target,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::AccountCursor<'a>
    where
        Self: 'a;

    type StorageCursor<'a>
        = <HashedPostStateCursorFactory<
        &'a Provider::Target,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::StorageCursor<'a>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        Ok(HashedPostStateCursor::new_account(
            self.provider().hashed_account_cursor()?,
            &overlay.input().state,
        ))
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        Ok(HashedPostStateCursor::new_storage(
            self.provider().hashed_storage_cursor(hashed_address)?,
            &overlay.input().state,
            hashed_address,
        ))
    }
}

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
}

type StateTrieOverlayCache = Arc<DashMap<(BlockHash, BlockHash, bool), StateTrieOverlay>>;

#[derive(Clone, Debug)]
struct CachedExecutionOverlay {
    overlay: Arc<ExecutionOverlay>,
    historical_fallback: Option<HistoricalFallback>,
}

#[derive(Clone, Copy, Debug)]
struct HistoricalFallback {
    block_number: BlockNumber,
    account_history_block_number: Option<BlockNumber>,
    storage_history_block_number: Option<BlockNumber>,
}

#[doc(hidden)]
#[derive(Debug)]
pub struct OwnedProvider<Provider>(Provider);

impl<Provider> Deref for OwnedProvider<Provider> {
    type Target = Provider;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

fn into_database_error(error: ProviderError) -> DatabaseError {
    match error {
        ProviderError::Database(error) => error,
        error => DatabaseError::Other(error.to_string()),
    }
}

impl<Provider, N: NodePrimitives> StateTrieCursorFactory for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + HashedCursorFactory
        + TrieCursorFactory
        + StateTrieCursorFactory
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    type AccountCursor<'a>
        =
        InMemoryStateTrieCursor<'a, <Provider::Target as StateTrieCursorFactory>::AccountCursor<'a>>
    where
        Self: 'a;
    type StorageCursor<'a>
        =
        InMemoryStateTrieCursor<'a, <Provider::Target as StateTrieCursorFactory>::StorageCursor<'a>>
    where
        Self: 'a;
    fn state_trie_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        let cursor = self.provider().state_trie_account_cursor()?;
        Ok(InMemoryStateTrieCursor::new(cursor, &overlay.input().state_trie.account_nodes))
    }
    fn state_trie_storage_cursor(
        &self,
        address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay(true).map_err(into_database_error)?;
        let cursor = self.provider().state_trie_storage_cursor(address)?;
        Ok(InMemoryStateTrieCursor::new_storage(cursor, &overlay.input().state_trie, address))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionOverlay, OverlayManager};
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_db_api::{
        models::{
            storage_sharded_key::StorageShardedKey, AccountBeforeTx, BlockNumberAddress, ShardedKey,
        },
        tables,
        transaction::DbTxMut,
        BlockNumberList,
    };
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{
        updates::TrieUpdatesSorted, BranchNodeCompact, ComputedTrieData, HashedPostState,
        HashedStorage,
    };
    use revm::{bytecode::Bytecode as RevmBytecode, state::AccountInfo};

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
        overlay.input().state.accounts.iter().map(|(key, _)| *key).collect()
    }

    fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
        overlay.input().nodes.account_nodes_ref().iter().map(|(path, _)| *path).collect()
    }

    #[cfg(feature = "state-trie-db")]
    #[test]
    fn complete_trie_cursors_merge_cached_updates_and_deletions() {
        use reth_trie::TrieAccount;

        for warm_parent in [false, true] {
            let (factory, mut blocks) = setup_frontiers(0, 0);
            let address = Address::with_last_byte(1);
            let removed = Address::with_last_byte(2);
            let hashed_address = alloy_primitives::keccak256(address);
            let path = Nibbles::unpack(hashed_address);
            let removed_path = Nibbles::unpack(alloy_primitives::keccak256(removed));
            let slot = B256::with_last_byte(1);
            let inherited_slot = B256::with_last_byte(2);
            let slot_path = Nibbles::unpack(alloy_primitives::keccak256(slot));
            let inherited_path = Nibbles::unpack(alloy_primitives::keccak256(inherited_slot));
            let account = |balance| StateTrieNode::Leaf {
                short_key_len: 63,
                value: TrieAccount { balance: U256::from(balance), ..Default::default() },
            };
            let storage =
                |value| StateTrieNode::Leaf { short_key_len: 63, value: U256::from(value) };
            let updates = |mut accounts: Vec<_>, mut slots: Vec<_>| {
                accounts.sort_unstable_by_key(|(p, _)| *p);
                slots.sort_unstable_by_key(|(p, _)| *p);
                StateTrieUpdatesSorted {
                    account_nodes: accounts,
                    storage_tries: std::iter::once((hashed_address, slots)).collect(),
                }
            };
            let rw = factory.provider_rw().unwrap();
            rw.write_state_trie_updates(&updates(
                vec![(path, Some(account(10))), (removed_path, Some(account(10)))],
                vec![(slot_path, Some(storage(10)))],
            ))
            .unwrap();
            rw.commit().unwrap();
            blocks[1].state_trie_updates = Some(Arc::new(updates(
                vec![(path, Some(account(20)))],
                vec![(slot_path, Some(storage(20))), (inherited_path, Some(storage(22)))],
            )));
            blocks[2].state_trie_updates = Some(Arc::new(updates(
                vec![(path, Some(account(30))), (removed_path, None)],
                vec![(slot_path, None)],
            )));
            let manager = OverlayManager::default();
            for block in &blocks[1..=2] {
                manager.insert_block(block.clone());
            }
            if warm_parent {
                let parent = OverlayStateProviderFactory::new(
                    factory.clone(),
                    manager.overlay_builder(blocks[1].recovered_block().hash()),
                );
                let provider = parent.database_provider_ro().unwrap();
                assert_eq!(
                    provider.state_trie_account_cursor().unwrap().get(path).unwrap(),
                    Some(account(20))
                );
            }
            let child = OverlayStateProviderFactory::new(
                factory,
                manager.overlay_builder(blocks[2].recovered_block().hash()),
            );
            let provider = child.database_provider_ro().unwrap();
            let mut accounts = provider.state_trie_account_cursor().unwrap();
            assert_eq!(accounts.get(path).unwrap(), Some(account(30)));
            assert_eq!(accounts.get(removed_path).unwrap(), None);
            let mut slots = provider.state_trie_storage_cursor(hashed_address).unwrap();
            assert_eq!(slots.get(slot_path).unwrap(), None);
            assert_eq!(slots.get(inherited_path).unwrap(), Some(storage(22)));
            assert!(!provider.state_trie_overlay(false).unwrap().skipped_for_reused_sparse_trie());
        }
    }

    #[cfg(feature = "state-trie-db")]
    #[test]
    fn execution_reads_overlay_bundle_state_on_complete_tables() {
        use reth_trie::TrieAccount;
        use revm::database::BundleState;

        let (factory, mut blocks) = setup_frontiers(0, 1);
        let address = Address::with_last_byte(1);
        let db_address = Address::with_last_byte(2);
        let slot = B256::with_last_byte(1);
        let db_slot = B256::with_last_byte(2);
        let db_account = TrieAccount { balance: U256::from(10), ..Default::default() };
        let rw = factory.provider_rw().unwrap();
        let updates = reth_trie::StateTrieUpdatesSorted {
            account_nodes: [address, db_address]
                .into_iter()
                .map(|address| {
                    (
                        Nibbles::unpack(alloy_primitives::keccak256(address)),
                        Some(StateTrieNode::Leaf { short_key_len: 63, value: db_account.clone() }),
                    )
                })
                .collect(),
            storage_tries: std::iter::once((
                alloy_primitives::keccak256(address),
                [slot, db_slot]
                    .into_iter()
                    .map(|slot| {
                        (
                            Nibbles::unpack(alloy_primitives::keccak256(slot)),
                            Some(StateTrieNode::Leaf { short_key_len: 63, value: U256::from(10) }),
                        )
                    })
                    .collect(),
            ))
            .collect(),
        };
        rw.write_state_trie_updates(&updates).unwrap();
        rw.commit().unwrap();

        let account = AccountInfo { balance: U256::from(20), ..Default::default() };
        Arc::make_mut(&mut blocks[1].execution_output).state = BundleState::builder(1..=1)
            .state_present_account_info(address, account.clone())
            .state_storage(
                address,
                std::iter::once((U256::from_be_bytes(slot.0), (U256::from(10), U256::ZERO)))
                    .collect(),
            )
            .build();
        let manager = OverlayManager::default();
        manager.insert_block(blocks[1].clone());
        let factory = OverlayStateProviderFactory::new(
            factory,
            manager.overlay_builder(blocks[1].recovered_block().hash()),
        );
        let provider = factory.database_provider_ro().unwrap();

        assert_eq!(provider.basic_account(&address).unwrap(), Some(Account::from(account)));
        assert_eq!(provider.basic_account(&db_address).unwrap(), Some(db_account.into()));
        assert_eq!(provider.basic_account(&Address::ZERO).unwrap(), None);
        assert_eq!(provider.storage(address, slot).unwrap(), Some(U256::ZERO));
        assert_eq!(provider.storage(address, db_slot).unwrap(), Some(U256::from(10)));
        assert_eq!(provider.storage(address, B256::ZERO).unwrap(), None);
        assert_eq!(
            provider.storage_batch(address, &[db_slot, slot, B256::ZERO, db_slot]).unwrap(),
            vec![Some(U256::from(10)), Some(U256::ZERO), None, Some(U256::from(10))]
        );
        assert!(provider.execution_overlay.get().is_some());
        assert!(provider.state_trie_overlay.get().is_none());
        assert!(provider.state_trie_overlay_with_trie_changesets.get().is_none());
        assert!(factory.state_trie_overlay_cache.is_empty());
    }

    #[test]
    fn overlay_cache_is_keyed_by_both_durable_frontiers() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory.clone(),
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );

        let provider = state_provider_factory.database_provider_ro().unwrap();
        let first = provider.state_trie_overlay(false).unwrap().clone();
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

        let provider = state_provider_factory.database_provider_ro().unwrap();
        let second = provider.state_trie_overlay(false).unwrap().clone();
        assert_eq!(account_keys(&second), vec![B256::with_last_byte(4)]);
        assert_eq!(account_node_paths(&second), vec![Nibbles::from_nibbles([4])]);
        assert_eq!(state_provider_factory.state_trie_overlay_cache.len(), 2);
    }

    #[cfg(all(feature = "legacy-trie-rocksdb", not(feature = "state-trie-db")))]
    #[test]
    fn legacy_rocksdb_execution_and_proof_cursors_share_backend() {
        use reth_provider::{StateWriter, TrieWriter};
        use reth_trie::{hashed_cursor::HashedCursor, trie_cursor::TrieCursor};
        let (factory, blocks) = setup_frontiers(0, 0);
        let address = Address::with_last_byte(7);
        let slot = B256::with_last_byte(8);
        let hashed_address = alloy_primitives::keccak256(address);
        let hashed_slot = alloy_primitives::keccak256(slot);
        let account = Account { nonce: 5, ..Default::default() };
        let branch = BranchNodeCompact::new(3, 0, 0, vec![], None);
        let path = Nibbles::from_nibbles([1]);
        let rw = factory.provider_rw().unwrap();
        rw.write_hashed_state(
            &HashedPostState::default()
                .with_accounts([(hashed_address, Some(account))])
                .with_storages([(
                    hashed_address,
                    HashedStorage::from_iter([(hashed_slot, U256::from(42))]),
                )])
                .into_sorted(),
        )
        .unwrap();
        rw.write_trie_updates_sorted(&TrieUpdatesSorted::new(
            vec![(path, Some(branch.clone()))],
            Default::default(),
        ))
        .unwrap();
        // A backend mix-up must return distinguishable data, rather than pass on identical copies.
        rw.tx().put::<tables::HashedAccounts>(hashed_address, Account::default()).unwrap();
        rw.tx()
            .put::<tables::HashedStorages>(
                hashed_address,
                reth_primitives_traits::StorageEntry { key: hashed_slot, value: U256::from(99) },
            )
            .unwrap();
        rw.commit().unwrap();
        let state_factory = OverlayStateProviderFactory::<_, EthPrimitives>::new(
            factory.clone(),
            OverlayManager::default().overlay_builder(blocks[0].recovered_block().hash()),
        );
        let old = state_factory.database_provider_ro().unwrap();
        assert_eq!(old.basic_account(&address).unwrap(), Some(account));
        assert_eq!(old.storage(address, slot).unwrap(), Some(U256::from(42)));
        assert_eq!(
            old.storage_batch(address, &[slot, B256::ZERO, slot]).unwrap(),
            vec![Some(U256::from(42)), None, Some(U256::from(42))]
        );
        assert!(old.state_trie_overlay.get().is_none());
        assert!(old.state_trie_overlay_with_trie_changesets.get().is_none());
        assert!(state_factory.state_trie_overlay_cache.is_empty());
        assert_eq!(
            old.hashed_account_cursor().unwrap().seek(hashed_address).unwrap(),
            Some((hashed_address, account))
        );
        assert_eq!(
            old.hashed_storage_cursor(hashed_address).unwrap().seek(hashed_slot).unwrap(),
            Some((hashed_slot, U256::from(42)))
        );
        assert_eq!(
            old.account_trie_cursor().unwrap().seek_exact(path).unwrap(),
            Some((path, branch))
        );
        let rw = factory.provider_rw().unwrap();
        rw.write_hashed_state(
            &HashedPostState::default()
                .with_accounts([(hashed_address, None)])
                .with_storages([(
                    hashed_address,
                    HashedStorage::from_iter([(hashed_slot, U256::ZERO)]),
                )])
                .into_sorted(),
        )
        .unwrap();
        rw.write_trie_updates_sorted(&TrieUpdatesSorted::new(
            vec![(path, None)],
            Default::default(),
        ))
        .unwrap();
        rw.commit().unwrap();
        let fresh = state_factory.database_provider_ro().unwrap();
        assert_eq!(fresh.basic_account(&address).unwrap(), None);
        assert_eq!(fresh.storage(address, slot).unwrap(), None);
        assert_eq!(fresh.account_trie_cursor().unwrap().seek_exact(path).unwrap(), None);
        assert_eq!(old.basic_account(&address).unwrap(), Some(account));
        assert_eq!(old.storage(address, slot).unwrap(), Some(U256::from(42)));
    }

    #[test]
    fn overlays_are_computed_lazily() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory,
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );

        let provider = state_provider_factory.database_provider_ro().unwrap();

        assert!(provider.state_trie_overlay.get().is_none());
        assert!(provider.execution_overlay.get().is_none());
        assert!(state_provider_factory.state_trie_overlay_cache.is_empty());

        provider.basic_account(&Address::ZERO).unwrap();
        provider.storage(Address::ZERO, B256::ZERO).unwrap();
        assert!(provider.state_trie_overlay_with_trie_changesets.get().is_none());
        assert!(provider.execution_overlay.get().is_some());
        assert!(state_provider_factory.state_trie_overlay_cache.is_empty());

        provider.account_trie_cursor().unwrap();
        assert_eq!(state_provider_factory.state_trie_overlay_cache.len(), 1);
    }

    #[test]
    fn state_trie_overlay_cache_is_keyed_by_trie_changesets() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory,
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );
        let provider = state_provider_factory.database_provider_ro().unwrap();

        provider.state_trie_overlay(false).unwrap();
        provider.state_trie_overlay(true).unwrap();

        assert_eq!(state_provider_factory.state_trie_overlay_cache.len(), 2);
    }

    #[test]
    fn supplied_state_trie_overlay_is_available_in_both_modes() {
        let (factory, _) = setup_frontiers(1, 1);
        let provider = factory.provider().unwrap();
        let provider = OverlayStateProvider::<&_, EthPrimitives>::new_with_state_trie(
            &provider,
            StateTrieOverlay::new(TrieInputSorted::default()),
            false,
        );

        assert!(provider.state_trie_overlay(false).is_ok());
        assert!(provider.state_trie_overlay(true).is_ok());
    }

    #[test]
    fn lazy_overlays_survive_manager_block_removal() {
        let (factory, blocks) = setup_frontiers(1, 1);
        let manager = OverlayManager::default();
        for block in &blocks[2..=3] {
            manager.insert_block(block.clone());
        }
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory,
            manager.overlay_builder(blocks[3].recovered_block().hash()),
        );
        let provider = state_provider_factory.database_provider_ro().unwrap();

        manager.remove_blocks(blocks[2..=3].iter().map(|block| block.recovered_block().hash()));

        let (execution_overlay, _) = provider.execution_overlay().unwrap();
        assert_eq!(
            execution_overlay.block_hashes(),
            [blocks[2].recovered_block().num_hash(), blocks[3].recovered_block().num_hash()]
        );
        assert_eq!(
            account_keys(provider.state_trie_overlay(false).unwrap()),
            vec![B256::with_last_byte(3), B256::with_last_byte(4)]
        );
    }

    #[test]
    fn skipped_state_trie_overlay_is_not_cached_or_used_for_state_roots() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[4].clone());
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory,
            manager.overlay_builder(blocks[4].recovered_block().hash()),
        )
        .with_skip_overlay_for_reused_sparse_trie(blocks[3].recovered_block().hash());

        let provider = state_provider_factory.database_provider_ro().unwrap();
        assert!(provider.state_trie_overlay(false).unwrap().skipped_for_reused_sparse_trie());
        assert!(provider
            .state_trie_account_cursor()
            .unwrap()
            .get(Nibbles::new())
            .unwrap()
            .is_none());
        assert!(provider
            .state_trie_storage_cursor(B256::ZERO)
            .unwrap()
            .get(Nibbles::new())
            .unwrap()
            .is_none());
        assert!(state_provider_factory.state_trie_overlay_cache.is_empty());
        assert!(matches!(
            provider.state_root(HashedPostState::default()),
            Err(ProviderError::UnsupportedProvider)
        ));
        assert!(state_provider_factory.state_trie_overlay_cache.is_empty());
    }

    #[test]
    fn execution_overlay_readers_use_overlay_first() {
        let (factory, _) = setup_frontiers(1, 3);
        let address = Address::with_last_byte(1);
        let account_info = AccountInfo { nonce: 1, balance: U256::from(2), ..Default::default() };
        let block_hash = B256::with_last_byte(3);
        let storage_key = B256::with_last_byte(4);
        let storage_value = U256::from(5);
        let code_hash = B256::with_last_byte(6);
        let bytecode = RevmBytecode::new_raw([0x60, 0x01].into());
        let mut execution_overlay = ExecutionOverlay::default();
        execution_overlay.accounts_mut().insert(address, Some(account_info.clone()));
        execution_overlay.block_hashes_mut().push(BlockNumHash::new(1, block_hash));
        execution_overlay
            .storage_mut()
            .entry(address)
            .or_default()
            .insert(U256::from_be_bytes(storage_key.0), storage_value);
        execution_overlay.code_hashes_mut().insert(code_hash, bytecode.clone());
        let provider = OverlayStateProvider::<_, EthPrimitives>::new_with_execution(
            factory.provider().unwrap(),
            Arc::new(execution_overlay),
            false,
        );

        assert_eq!(provider.basic_account(&address).unwrap(), Some(Account::from(account_info)));
        assert!(provider.basic_account(&Address::with_last_byte(2)).unwrap().is_none());
        assert_eq!(provider.storage(address, storage_key).unwrap(), Some(storage_value));
        assert_eq!(provider.block_hash(1).unwrap(), Some(block_hash));
        assert_eq!(provider.canonical_hashes_range(1, 2).unwrap(), vec![block_hash]);
        assert_eq!(
            provider.bytecode_by_hash(&code_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(bytecode))
        );
    }

    #[test]
    #[allow(clippy::clone_on_copy)]
    fn historical_execution_reads_use_history_indexes() {
        let (factory, blocks) = setup_frontiers(1, 3);
        let address = Address::with_last_byte(1);
        let storage_key = B256::with_last_byte(2);
        let account = Account { balance: U256::from(10), ..Default::default() };
        let storage = U256::from(10);
        let provider_rw = factory.provider_rw().unwrap();

        provider_rw
            .tx_ref()
            .put::<tables::AccountsHistory>(
                ShardedKey { key: address, highest_block_number: u64::MAX },
                BlockNumberList::new([2]).unwrap(),
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::AccountChangeSets>(
                2,
                AccountBeforeTx { address, info: Some(account.clone()) },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::PlainAccountState>(
                address,
                Account { balance: U256::from(20), ..Default::default() },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::StoragesHistory>(
                StorageShardedKey {
                    address,
                    sharded_key: ShardedKey { key: storage_key, highest_block_number: u64::MAX },
                },
                BlockNumberList::new([2]).unwrap(),
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::StorageChangeSets>(
                BlockNumberAddress((2, address)),
                reth_primitives_traits::StorageEntry { key: storage_key, value: storage },
            )
            .unwrap();
        provider_rw
            .tx_ref()
            .put::<tables::PlainStorageState>(
                address,
                reth_primitives_traits::StorageEntry { key: storage_key, value: U256::from(20) },
            )
            .unwrap();
        provider_rw.commit().unwrap();

        let state_provider_factory = OverlayStateProviderFactory::<_, EthPrimitives>::new(
            factory,
            OverlayManager::default().overlay_builder(blocks[1].recovered_block().hash()),
        );
        let provider = state_provider_factory.database_provider_ro().unwrap();

        if cfg!(feature = "state-trie-db") {
            assert!(provider.basic_account(&address).is_err());
            assert!(provider.storage(address, storage_key).is_err());
        } else {
            assert_eq!(provider.basic_account(&address).unwrap(), Some(account));
            assert_eq!(provider.storage(address, storage_key).unwrap(), Some(storage));
        }
    }
}
