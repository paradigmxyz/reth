use alloy_primitives::{BlockNumber, B256};
use reth_db_api::DatabaseError;
use reth_errors::ProviderError;
use reth_stages_types::StageId;
use reth_storage_api::{DBProvider, DatabaseProviderFactory, StageCheckpointReader, TrieReader};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursor, InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    HashedPostState, HashedPostStateSorted, KeccakKeyHasher,
};
use reth_trie_db::{
    DatabaseAccountTrieCursor, DatabaseHashedCursorFactory, DatabaseHashedPostState,
    DatabaseStorageTrieCursor, DatabaseTrieCursorFactory,
};
use std::sync::Arc;

/// Factory for creating overlay state providers with optional reverts and overlays.
///
/// This factory allows building an `OverlayStateProvider` whose DB state has been reverted to a
/// particular block, and/or with additional overlay information added on top.
#[derive(Debug, Clone)]
pub struct OverlayStateProviderFactory<F> {
    /// The underlying database provider factory
    factory: F,
    /// Optional block number for collecting reverts
    block_number: Option<BlockNumber>,
    /// Optional trie overlay
    trie_overlay: Option<Arc<TrieUpdatesSorted>>,
    /// Optional hashed state overlay
    hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
}

impl<F> OverlayStateProviderFactory<F>
where
    F: DatabaseProviderFactory,
    F::Provider: Clone + TrieReader + StageCheckpointReader,
{
    /// Create a new overlay state provider factory
    pub const fn new(factory: F) -> Self {
        Self { factory, block_number: None, trie_overlay: None, hashed_state_overlay: None }
    }

    /// Set the block number for collecting reverts
    pub const fn with_block_number(mut self, block_number: Option<BlockNumber>) -> Self {
        self.block_number = block_number;
        self
    }

    /// Set the trie overlay
    pub fn with_trie_overlay(mut self, trie_overlay: Option<Arc<TrieUpdatesSorted>>) -> Self {
        self.trie_overlay = trie_overlay;
        self
    }

    /// Set the hashed state overlay
    pub fn with_hashed_state_overlay(
        mut self,
        hashed_state_overlay: Option<Arc<HashedPostStateSorted>>,
    ) -> Self {
        self.hashed_state_overlay = hashed_state_overlay;
        self
    }

    /// Validates that there are sufficient changesets to revert to the requested block number.
    ///
    /// Returns an error if the `MerkleChangeSets` checkpoint doesn't cover the requested block.
    fn validate_changesets_availability(
        &self,
        provider: &F::Provider,
        requested_block: BlockNumber,
    ) -> Result<(), ProviderError> {
        // Get the MerkleChangeSets stage checkpoint - let errors propagate as-is
        let checkpoint = provider.get_stage_checkpoint(StageId::MerkleChangeSets)?;

        // If there's no checkpoint at all or block range details are missing, we can't revert
        let available_range = checkpoint
            .and_then(|chk| {
                chk.merkle_changesets_stage_checkpoint()
                    .map(|stage_chk| stage_chk.block_range.from..=chk.block_number)
            })
            .ok_or_else(|| ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: 0..=0,
            })?;

        // Check if the requested block is within the available range
        if !available_range.contains(&requested_block) {
            return Err(ProviderError::InsufficientChangesets {
                requested: requested_block,
                available: available_range,
            });
        }

        Ok(())
    }

    /// Create a read-only [`OverlayStateProvider`].
    pub fn provider_ro(&self) -> Result<OverlayStateProvider<F::Provider>, ProviderError> {
        // Get a read-only provider
        let provider = self.factory.database_provider_ro()?;

        // If block_number is provided, collect reverts
        let (trie_updates, hashed_state) = if let Some(from_block) = self.block_number {
            // Validate that we have sufficient changesets for the requested block
            self.validate_changesets_availability(&provider, from_block)?;

            // Collect trie reverts
            let mut trie_updates_mut = provider.trie_reverts(from_block)?;

            // Collect state reverts using HashedPostState::from_reverts
            let reverted_state =
                HashedPostState::from_reverts::<KeccakKeyHasher>(provider.tx_ref(), from_block..)?;
            let mut hashed_state_mut = reverted_state.into_sorted();

            // Extend with overlays if provided
            if let Some(trie_overlay) = &self.trie_overlay {
                trie_updates_mut.extend_ref(trie_overlay);
            }

            if let Some(hashed_state_overlay) = &self.hashed_state_overlay {
                hashed_state_mut.extend_ref(hashed_state_overlay);
            }

            (Arc::new(trie_updates_mut), Arc::new(hashed_state_mut))
        } else {
            // If no block_number, use overlays directly or defaults
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
#[derive(Debug, Clone)]
pub struct OverlayStateProvider<Provider: DBProvider> {
    provider: Provider,
    trie_updates: Arc<TrieUpdatesSorted>,
    hashed_post_state: Arc<HashedPostStateSorted>,
}

impl<Provider> OverlayStateProvider<Provider>
where
    Provider: DBProvider + Clone,
{
    /// Create new overlay state provider. The `Provider` must be cloneable, which generally means
    /// it should be wrapped in an `Arc`.
    pub fn new(
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
        let cursor = DatabaseAccountTrieCursor::from_tx(self.provider.tx_ref())?;
        Ok(InMemoryTrieCursor::new(Some(cursor), self.trie_updates.account_nodes_ref()))
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor<'_>, DatabaseError> {
        let cursor = DatabaseStorageTrieCursor::from_tx(self.provider.tx_ref(), hashed_address)?;
        Ok(InMemoryTrieCursor::new_storage(Some(cursor), &self.trie_updates, hashed_address))
    }
}

impl<Provider> HashedCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider + Clone,
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
