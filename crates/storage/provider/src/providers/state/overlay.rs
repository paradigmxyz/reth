use alloy_primitives::{BlockNumber, B256};
use reth_storage_api::DBProvider;
use reth_storage_errors::{ProviderResult, db::DatabaseError};
use reth_trie::{
    hashed_cursor::{HashedCursorFactory, HashedPostStateCursorFactory},
    trie_cursor::{InMemoryTrieCursorFactory, TrieCursorFactory},
    updates::TrieUpdatesSorted,
    HashedPostStateSorted,
};
use reth_trie_db::{DatabaseHashedCursorFactory, DatabaseTrieCursorFactory};
use std::sync::Arc;

/// State provider with in-memory overlay from trie updates and hashed post state.
///
/// This provider uses in-memory trie updates and hashed post state as an overlay
/// on top of a database provider, implementing [`TrieCursorFactory`] and [`HashedCursorFactory`]
/// using the in-memory overlay factories.
#[derive(Debug, Clone)]
pub struct OverlayStateProvider<Provider: DBProvider> {
    /// The in-memory trie cursor factory that wraps the database cursor factory.
    trie_cursor_factory:
        InMemoryTrieCursorFactory<DatabaseTrieCursorFactory<Arc<Provider::Tx>>, Arc<TrieUpdatesSorted>>,
    /// The hashed cursor factory that wraps the database cursor factory.
    hashed_cursor_factory: HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<Arc<Provider::Tx>>,
        Arc<HashedPostStateSorted>,
    >,
}

impl<Provider> OverlayStateProvider<Provider>
where
    Provider: DBProvider,
{
    /// Create new overlay state provider.
    pub fn new(
        provider: Provider,
        trie_updates: Arc<TrieUpdatesSorted>,
        hashed_post_state: Arc<HashedPostStateSorted>,
    ) -> Self {
        // Wrap the transaction in an Arc so it can be shared between both cursor factories
        let tx = Arc::new(provider.into_tx());

        // Create the trie cursor factory
        let db_trie_cursor_factory = DatabaseTrieCursorFactory::new(tx.clone());
        let trie_cursor_factory =
            InMemoryTrieCursorFactory::new(db_trie_cursor_factory, trie_updates);

        // Create the hashed cursor factory
        let db_hashed_cursor_factory = DatabaseHashedCursorFactory::new(tx);
        let hashed_cursor_factory =
            HashedPostStateCursorFactory::new(db_hashed_cursor_factory, hashed_post_state);

        Self { trie_cursor_factory, hashed_cursor_factory }
    }

    /// Create a new overlay state provider by first reverting hashed state and trie to the given
    /// `block_number`, and then applying the provided `HashedPostStateSorted` and
    /// `TrieUpdatesSorted` on top of those reverts.
    pub fn with_reverts(
        _provider: Provider,
        _trie_updates: Arc<TrieUpdatesSorted>,
        _hashed_post_state: Arc<HashedPostStateSorted>,
        _block_number: BlockNumber,
    ) -> ProviderResult<Self> {
        todo!()
    }
}

impl<Provider> TrieCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider,
    InMemoryTrieCursorFactory<DatabaseTrieCursorFactory<Arc<Provider::Tx>>, Arc<TrieUpdatesSorted>>:
        TrieCursorFactory,
{
    type AccountTrieCursor = <InMemoryTrieCursorFactory<
        DatabaseTrieCursorFactory<Arc<Provider::Tx>>,
        Arc<TrieUpdatesSorted>,
    > as TrieCursorFactory>::AccountTrieCursor;

    type StorageTrieCursor = <InMemoryTrieCursorFactory<
        DatabaseTrieCursorFactory<Arc<Provider::Tx>>,
        Arc<TrieUpdatesSorted>,
    > as TrieCursorFactory>::StorageTrieCursor;

    fn account_trie_cursor(&self) -> Result<Self::AccountTrieCursor, DatabaseError> {
        self.trie_cursor_factory.account_trie_cursor()
    }

    fn storage_trie_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageTrieCursor, DatabaseError> {
        self.trie_cursor_factory.storage_trie_cursor(hashed_address)
    }
}

impl<Provider> HashedCursorFactory for OverlayStateProvider<Provider>
where
    Provider: DBProvider,
    HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<Arc<Provider::Tx>>,
        Arc<HashedPostStateSorted>,
    >: HashedCursorFactory,
{
    type AccountCursor = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<Arc<Provider::Tx>>,
        Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::AccountCursor;

    type StorageCursor = <HashedPostStateCursorFactory<
        DatabaseHashedCursorFactory<Arc<Provider::Tx>>,
        Arc<HashedPostStateSorted>,
    > as HashedCursorFactory>::StorageCursor;

    fn hashed_account_cursor(&self) -> Result<Self::AccountCursor, DatabaseError> {
        self.hashed_cursor_factory.hashed_account_cursor()
    }

    fn hashed_storage_cursor(
        &self,
        hashed_address: B256,
    ) -> Result<Self::StorageCursor, DatabaseError> {
        self.hashed_cursor_factory.hashed_storage_cursor(hashed_address)
    }
}
