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
use reth_storage_api::{
    AccountReader, BlockHashReader, BlockNumReader, BytecodeReader, ChangeSetReader, DBProvider,
    DatabaseProviderFactory, DatabaseProviderROFactory, DbTxProvider, HashedPostStateProvider,
    PruneCheckpointReader, StageCheckpointReader, StateProofProvider, StateProvider,
    StateRootProvider, StorageChangeSetReader, StorageRootProvider, StorageSettingsCache,
};
use reth_trie::{
    hashed_cursor::{
        zero_destroyed_account_storage, HashedCursorFactory, HashedPostStateCursorFactory,
    },
    proof::{Proof, StorageProof as TrieStorageProof},
    trie_cursor::{
        InMemoryTrieCursor, InMemoryTrieCursorFactory, TrieCursor, TrieCursorFactory,
        TrieStorageCursor,
    },
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, ExecutionWitnessMode, HashedPostState, HashedPostStateSorted, HashedStorage,
    KeccakKeyHasher, MultiProof, MultiProofTargets, StateRoot, StorageMultiProof, StorageProof,
    StorageRoot, TrieInput, TrieInputSorted,
};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseHashedCursorFactory, DatabaseProof, DatabaseStateRoot,
    DatabaseStorageProof, DatabaseStorageRoot, DatabaseStorageTrieCursor,
    DatabaseTrieCursorFactory, LegacyKeyAdapter, PackedAccountsTrie, PackedKeyAdapter,
    PackedStoragesTrie,
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
    /// A cache mapping `(state_trie_tip, finish_tip)` to [`StateTrieOverlay`].
    ///
    /// Under partial persistence the overlay depends on both durable frontiers, so both hashes are
    /// part of the cache key.
    state_trie_overlay_cache: StateTrieOverlayCache,
    /// A cache which maps durable frontier pairs to [`ExecutionOverlay`]s.
    execution_overlay_cache: ExecutionOverlayCache,
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
            execution_overlay_cache: Default::default(),
            metrics: Default::default(),
        }
    }

    /// Skips managed overlay construction when this factory is used by a task that reused a sparse
    /// trie covering both durable frontiers through the parent.
    pub fn with_skip_overlay_for_reused_sparse_trie(mut self, anchor_hash: B256) -> Self {
        self.overlay_builder =
            self.overlay_builder.with_skip_overlay_for_reused_sparse_trie(anchor_hash);
        self.state_trie_overlay_cache = Default::default();
        self.execution_overlay_cache = Default::default();
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
        Ok(OverlayStateProvider::new(
            provider,
            self.overlay_builder.clone(),
            Arc::clone(&self.state_trie_overlay_cache),
            Arc::clone(&self.execution_overlay_cache),
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
    execution_overlay_cache: ExecutionOverlayCache,
    metrics: OverlayStateProviderFactoryMetrics,
    state_trie_overlay: OnceCell<StateTrieOverlay>,
    execution_overlay: OnceCell<Arc<ExecutionOverlay>>,
    is_v2: bool,
}

impl<Provider, N: NodePrimitives> OverlayStateProvider<OwnedProvider<Provider>, N> {
    const fn new(
        provider: Provider,
        overlay_builder: OverlayBuilder<N>,
        state_trie_overlay_cache: StateTrieOverlayCache,
        execution_overlay_cache: ExecutionOverlayCache,
        metrics: OverlayStateProviderFactoryMetrics,
        is_v2: bool,
    ) -> Self {
        Self {
            provider: OwnedProvider(provider),
            overlay_builder: Some(overlay_builder),
            state_trie_overlay_cache,
            execution_overlay_cache,
            metrics,
            state_trie_overlay: OnceCell::new(),
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
            execution_overlay_cache: Default::default(),
            metrics: Default::default(),
            state_trie_overlay: OnceCell::new(),
            execution_overlay: OnceCell::from(execution_overlay),
            is_v2,
        }
    }
}

impl<'a, Provider, N: NodePrimitives> OverlayStateProvider<&'a Provider, N> {
    pub(crate) fn new_with_state_trie(
        provider: &'a Provider,
        state_trie_overlay: StateTrieOverlay,
        is_v2: bool,
    ) -> Self {
        Self {
            provider,
            overlay_builder: None,
            state_trie_overlay_cache: Default::default(),
            execution_overlay_cache: Default::default(),
            metrics: Default::default(),
            state_trie_overlay: OnceCell::from(state_trie_overlay),
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

    fn state_trie_overlay(&self) -> ProviderResult<&StateTrieOverlay>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        if let Some(overlay) = self.state_trie_overlay.get() {
            return Ok(overlay)
        }

        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(self.provider())?;
        let overlay = match self
            .state_trie_overlay_cache
            .entry((state_trie_tip_block.hash, finish_tip_block.hash))
        {
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
                    )?;
                if !overlay.skipped_for_reused_sparse_trie() {
                    entry.insert(overlay.clone());
                }
                overlay
            }
        };
        let _ = self.state_trie_overlay.set(overlay);
        Ok(self.state_trie_overlay.get().expect("state trie overlay was just initialized"))
    }

    fn build_overlay(&self, input: TrieInputSorted) -> ProviderResult<TrieInputSorted>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader
            + StorageSettingsCache,
    {
        let overlay = self.state_trie_overlay()?;
        if overlay.skipped_for_reused_sparse_trie() {
            return Err(ProviderError::UnsupportedProvider)
        }
        let TrieInputSorted { nodes: input_nodes, state: input_state, prefix_sets } = input;
        let mut nodes = Arc::clone(&overlay.trie_updates);
        let mut state = Arc::clone(&overlay.hashed_post_state);

        if !input_nodes.is_empty() {
            Arc::make_mut(&mut nodes).extend_ref_and_sort(&input_nodes);
        }
        if !input_state.is_empty() {
            Arc::make_mut(&mut state).extend_ref_and_sort(&input_state);
        }

        Ok(TrieInputSorted::new(nodes, state, prefix_sets))
    }

    fn execution_overlay(&self) -> ProviderResult<&Arc<ExecutionOverlay>>
    where
        Provider::Target: StageCheckpointReader
            + PruneCheckpointReader
            + ChangeSetReader
            + StorageChangeSetReader
            + DBProvider
            + BlockNumReader,
    {
        if let Some(overlay) = self.execution_overlay.get() {
            return Ok(overlay)
        }

        let (state_trie_tip_block, finish_tip_block) = database_state_frontiers(self.provider())?;
        let overlay = match self
            .execution_overlay_cache
            .entry((state_trie_tip_block.hash, finish_tip_block.hash))
        {
            dashmap::Entry::Occupied(entry) => entry.get().clone(),
            dashmap::Entry::Vacant(entry) => {
                self.metrics.execution_overlay_cache_misses.increment(1);
                let overlay = self
                    .overlay_builder
                    .as_ref()
                    .expect("execution overlay must be initialized or lazily resolvable")
                    .build_execution_overlay_at_frontiers(
                        self.provider(),
                        state_trie_tip_block,
                        finish_tip_block,
                    )?;
                entry.insert(overlay.clone());
                overlay
            }
        };
        let _ = self.execution_overlay.set(overlay);
        Ok(self.execution_overlay.get().expect("execution overlay was just initialized"))
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
        + StorageSettingsCache
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        let overlay = self.execution_overlay()?;
        if let Some(account) = overlay.accounts().get(address) {
            return Ok(account.as_ref().map(Account::from))
        }
        if self.provider().cached_storage_settings().use_hashed_state() {
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
        + Sized
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader,
{
    fn block_hash(&self, number: BlockNumber) -> ProviderResult<Option<B256>> {
        let overlay = self.execution_overlay()?;
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
        let overlay = self.execution_overlay()?;
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
        let overlay = self.execution_overlay()?;
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
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(
                TrieInput::from_state(hashed_state),
            ))?;
            Ok(<DbStateRoot<'_, _, A>>::overlay_root_from_nodes(self.provider().tx(), input)?)
        })
    }

    fn state_root_from_nodes(&self, input: TrieInput) -> ProviderResult<B256> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(input))?;
            Ok(<DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root_from_nodes(
                self.provider().tx(),
                input,
            )?)
        })
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(
                TrieInput::from_state(hashed_state),
            ))?;
            Ok(<DbStateRoot<'_, _, A>>::overlay_root_from_nodes_with_updates(
                self.provider().tx(),
                input,
            )?)
        })
    }

    fn state_root_from_nodes_with_updates(
        &self,
        input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(input))?;
            Ok(
                <DbStateRoot<'_, _, A> as DatabaseStateRoot<_>>::overlay_root_from_nodes_with_updates(
                    self.provider().tx(),
                    input,
                )?,
            )
        })
    }
}

impl<Provider, N: NodePrimitives> StorageRootProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
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
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(
                TrieInput::from_state(HashedPostState::from_hashed_storage(
                    alloy_primitives::keccak256(address),
                    hashed_storage,
                )),
            ))?;
            let hashed_storage = input
                .state
                .account_storages()
                .get(&alloy_primitives::keccak256(address))
                .cloned()
                .unwrap_or_default()
                .into();
            <DbStorageRoot<'_, _, A>>::overlay_root(self.provider().tx(), address, hashed_storage)
                .map_err(|err| ProviderError::Database(err.into()))
        })
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageProof> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(
                TrieInput::from_state(HashedPostState::from_hashed_storage(
                    alloy_primitives::keccak256(address),
                    hashed_storage,
                )),
            ))?;
            let hashed_storage = input
                .state
                .account_storages()
                .get(&alloy_primitives::keccak256(address))
                .cloned()
                .unwrap_or_default()
                .into();
            <DbStorageProof<'_, _, A>>::overlay_storage_proof(
                self.provider().tx(),
                address,
                slot,
                hashed_storage,
            )
            .map_err(ProviderError::from)
        })
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let input = self.build_overlay(TrieInputSorted::from_unsorted(
                TrieInput::from_state(HashedPostState::from_hashed_storage(
                    alloy_primitives::keccak256(address),
                    hashed_storage,
                )),
            ))?;
            let hashed_storage = input
                .state
                .account_storages()
                .get(&alloy_primitives::keccak256(address))
                .cloned()
                .unwrap_or_default()
                .into();
            <DbStorageProof<'_, _, A>>::overlay_storage_multiproof(
                self.provider().tx(),
                address,
                slots,
                hashed_storage,
            )
            .map_err(ProviderError::from)
        })
    }
}

impl<Provider, N: NodePrimitives> StateProofProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
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
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let TrieInputSorted { nodes, state, prefix_sets } =
                self.build_overlay(TrieInputSorted::from_unsorted(input))?;
            let input = TrieInput::new(
                Arc::unwrap_or_clone(nodes).into(),
                Arc::unwrap_or_clone(state).into(),
                prefix_sets,
            );
            let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(self.provider().tx());
            proof.overlay_account_proof(input, address, slots).map_err(ProviderError::from)
        })
    }

    fn multiproof(
        &self,
        input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let TrieInputSorted { nodes, state, prefix_sets } =
                self.build_overlay(TrieInputSorted::from_unsorted(input))?;
            let input = TrieInput::new(
                Arc::unwrap_or_clone(nodes).into(),
                Arc::unwrap_or_clone(state).into(),
                prefix_sets,
            );
            let proof = <DbProof<'_, _, A> as DatabaseProof>::from_tx(self.provider().tx());
            proof.overlay_multiproof(input, targets).map_err(ProviderError::from)
        })
    }

    fn witness(
        &self,
        input: TrieInput,
        target: HashedPostState,
        mode: ExecutionWitnessMode,
    ) -> ProviderResult<Vec<alloy_primitives::Bytes>> {
        reth_trie_db::with_adapter!(self.provider(), |A| {
            let TrieInputSorted { nodes, state, prefix_sets } =
                self.build_overlay(TrieInputSorted::from_unsorted(input))?;
            let witness = TrieWitness::new(
                InMemoryTrieCursorFactory::new(
                    DatabaseTrieCursorFactory::<_, A>::new(self.provider().tx()),
                    nodes.as_ref(),
                ),
                HashedPostStateCursorFactory::new(
                    DatabaseHashedCursorFactory::new(self.provider().tx()),
                    state.as_ref(),
                ),
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
        })
    }
}

impl<Provider, N: NodePrimitives> HashedPostStateProvider for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
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

        let overlay_state = self.build_overlay(TrieInputSorted::default())?.state;
        zero_destroyed_account_storage(
            &HashedPostStateCursorFactory::new(
                DatabaseHashedCursorFactory::new(self.provider().tx()),
                overlay_state.as_ref(),
            ),
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
        let overlay = self.execution_overlay()?;
        if let Some(value) = overlay.storage_value(address, U256::from_be_bytes(storage_key.0)) {
            return Ok(Some(value));
        }
        if self.provider().cached_storage_settings().use_hashed_state() {
            let hashed_address = alloy_primitives::keccak256(address);
            let hashed_slot = alloy_primitives::keccak256(storage_key);
            let mut cursor = self.provider().tx().cursor_dup_read::<tables::HashedStorages>()?;
            Ok(cursor
                .seek_by_key_subkey(hashed_address, hashed_slot)?
                .filter(|entry| entry.key == hashed_slot)
                .map(|entry| entry.value))
        } else {
            let mut cursor = self.provider().tx().cursor_dup_read::<tables::PlainStorageState>()?;
            if let Some(entry) = cursor.seek_by_key_subkey(address, storage_key)? &&
                entry.key == storage_key
            {
                return Ok(Some(entry.value))
            }
            Ok(None)
        }
    }
}

impl<Provider, N: NodePrimitives> TrieCursorFactory for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
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
        let overlay = self.state_trie_overlay().map_err(into_database_error)?;
        let cursor: Box<dyn TrieCursor + Send> = if self.is_v2 {
            Box::new(DatabaseAccountTrieCursor::<_, PackedKeyAdapter>::new(
                self.provider().tx().cursor_read::<PackedAccountsTrie>()?,
            ))
        } else {
            Box::new(DatabaseAccountTrieCursor::<_, LegacyKeyAdapter>::new(
                self.provider().tx().cursor_read::<tables::AccountsTrie>()?,
            ))
        };
        Ok(InMemoryTrieCursor::new_account(cursor, &overlay.trie_updates))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay().map_err(into_database_error)?;
        let cursor: Box<dyn TrieStorageCursor + Send> = if self.is_v2 {
            Box::new(DatabaseStorageTrieCursor::<_, PackedKeyAdapter>::new(
                self.provider().tx().cursor_dup_read::<PackedStoragesTrie>()?,
                hashed_address,
            ))
        } else {
            Box::new(DatabaseStorageTrieCursor::<_, LegacyKeyAdapter>::new(
                self.provider().tx().cursor_dup_read::<tables::StoragesTrie>()?,
                hashed_address,
            ))
        };
        Ok(InMemoryTrieCursor::new_storage(cursor, &overlay.trie_updates, hashed_address))
    }
}

impl<Provider, N: NodePrimitives> HashedCursorFactory for OverlayStateProvider<Provider, N>
where
    Provider: Deref,
    Provider::Target: DBProvider
        + StageCheckpointReader
        + PruneCheckpointReader
        + ChangeSetReader
        + StorageChangeSetReader
        + BlockNumReader
        + StorageSettingsCache,
{
    type AccountCursor<'a>
        = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<&'a <Provider::Target as DbTxProvider>::Tx>,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::AccountCursor<'a>
    where
        Self: 'a;

    type StorageCursor<'a>
        = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<&'a <Provider::Target as DbTxProvider>::Tx>,
        &'a Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::StorageCursor<'a>
    where
        Self: 'a;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay().map_err(into_database_error)?;
        HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(self.provider().tx()),
            &overlay.hashed_post_state,
        )
        .hashed_account_cursor()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor<'_>, DatabaseError> {
        let overlay = self.state_trie_overlay().map_err(into_database_error)?;
        HashedPostStateCursorFactory::new(
            DatabaseHashedCursorFactory::new(self.provider().tx()),
            &overlay.hashed_post_state,
        )
        .hashed_storage_cursor(hashed_address)
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
    /// Number of cache misses when fetching execution overlays.
    execution_overlay_cache_misses: Counter,
}

type StateTrieOverlayCache = Arc<DashMap<(BlockHash, BlockHash), StateTrieOverlay>>;
type ExecutionOverlayCache = Arc<DashMap<(BlockHash, BlockHash), Arc<ExecutionOverlay>>>;

type DbStateRoot<'a, TX, A> =
    StateRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type DbStorageRoot<'a, TX, A> =
    StorageRoot<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;
type DbStorageProof<'a, TX, A> = TrieStorageProof<
    'static,
    DatabaseTrieCursorFactory<&'a TX, A>,
    DatabaseHashedCursorFactory<&'a TX>,
>;
type DbProof<'a, TX, A> =
    Proof<DatabaseTrieCursorFactory<&'a TX, A>, DatabaseHashedCursorFactory<&'a TX>>;

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ExecutionOverlay, OverlayManager};
    use alloy_eips::BlockNumHash;
    use alloy_primitives::{Address, U256};
    use reth_chain_state::{test_utils::TestBlockBuilder, ExecutedBlock};
    use reth_primitives_traits::Account;
    use reth_provider::{
        test_utils::{create_test_provider_factory, MockNodeTypesWithDB},
        BlockWriter, ProviderFactory,
    };
    use reth_stages_types::{FinishCheckpoint, StageCheckpoint, StageId};
    use reth_storage_api::StageCheckpointWriter;
    use reth_trie::{
        updates::TrieUpdatesSorted, BranchNodeCompact, ComputedTrieData, HashedPostState,
        HashedStorage, Nibbles,
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
        overlay.hashed_post_state.accounts.iter().map(|(key, _)| *key).collect()
    }

    fn account_node_paths(overlay: &StateTrieOverlay) -> Vec<Nibbles> {
        overlay.trie_updates.account_nodes_ref().iter().map(|(path, _)| *path).collect()
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
        let first = provider.state_trie_overlay().unwrap().clone();
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
        let second = provider.state_trie_overlay().unwrap().clone();
        assert_eq!(account_keys(&second), vec![B256::with_last_byte(4)]);
        assert_eq!(account_node_paths(&second), vec![Nibbles::from_nibbles([4])]);
        assert_eq!(state_provider_factory.state_trie_overlay_cache.len(), 2);
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
        assert!(state_provider_factory.execution_overlay_cache.is_empty());

        provider.basic_account(&Address::ZERO).unwrap();
        let execution_overlay = Arc::clone(provider.execution_overlay.get().unwrap());
        assert!(provider.state_trie_overlay.get().is_none());
        assert!(provider.execution_overlay.get().is_some());
        assert!(state_provider_factory.state_trie_overlay_cache.is_empty());
        assert_eq!(state_provider_factory.execution_overlay_cache.len(), 1);
        let cached_overlay = state_provider_factory.execution_overlay_cache.iter().next().unwrap();
        assert!(Arc::ptr_eq(&execution_overlay, cached_overlay.value()));

        provider.account_trie_cursor().unwrap();
        assert_eq!(state_provider_factory.state_trie_overlay_cache.len(), 1);
        assert_eq!(state_provider_factory.execution_overlay_cache.len(), 1);
    }

    #[test]
    fn skipped_state_trie_overlay_is_not_cached_or_used_for_state_roots() {
        let (factory, blocks) = setup_frontiers(3, 3);
        let manager = OverlayManager::default();
        manager.insert_block(blocks[4].clone());
        let state_provider_factory = OverlayStateProviderFactory::new(
            factory,
            manager
                .overlay_builder(blocks[4].recovered_block().hash())
                .with_skip_overlay_for_reused_sparse_trie(blocks[3].recovered_block().hash()),
        );

        let provider = state_provider_factory.database_provider_ro().unwrap();
        assert!(provider.state_trie_overlay().unwrap().skipped_for_reused_sparse_trie());
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
        assert_eq!(provider.block_hash(1).unwrap(), Some(block_hash));
        assert_eq!(provider.canonical_hashes_range(1, 2).unwrap(), vec![block_hash]);
        assert_eq!(provider.storage(address, storage_key).unwrap(), Some(storage_value));
        assert_eq!(
            provider.bytecode_by_hash(&code_hash).unwrap(),
            Some(reth_primitives_traits::Bytecode(bytecode))
        );
    }
}
