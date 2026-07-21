use crate::tree::TxPoolPrewarmCacheSnapshot as Snapshot;
use alloy_primitives::{
    map::{AddressMap, B256Map, HashMap},
    Address, BlockNumber, StorageKey, StorageValue, B256,
};
use reth_primitives_traits::{Account, Bytecode};
use reth_provider::StateProviderBox;
use reth_revm::database::EvmStateProvider;
use std::cell::RefCell;

/// The read-through cache for the txpool prewarming worker.
///
/// Wrapped through [`Self::state_provider`] and passed to the EVM to collect state reads against
/// the pre-state for the given parent hash block.
///
/// Supports creating a [`Snapshot`] of the current cache state.
#[derive(Debug, Default)]
pub(super) struct Cache {
    // NOTE: RefCell is required for implementing the EvmStateProvider trait because its getters
    // are defined accepting &self.
    inner: RefCell<CacheInner>,
    /// The hash of the parent block, at which state this cache is based.
    parent_hash: Option<B256>,
}

#[derive(Debug, Default)]
struct CacheInner {
    accounts: AddressMap<Option<Account>>,
    storage: HashMap<(Address, StorageKey), StorageValue>,
    bytecodes: B256Map<Option<Bytecode>>,
    /// Whether entries were added since the last [`Cache::snapshot`] or [`Cache::reset`].
    dirty: bool,
}

impl Cache {
    /// Clears the cache and sets the parent hash to the given value.
    pub(super) fn reset(&mut self, parent_hash: B256) {
        self.parent_hash = Some(parent_hash);

        let mut cache = self.inner.borrow_mut();
        cache.accounts.clear();
        cache.storage.clear();
        cache.bytecodes.clear();
        cache.dirty = false;
    }

    /// Clones the current cache state into a [`Snapshot`] and marks the cache clean:
    /// [`Self::is_dirty`] returns `false` until the next read-through insert.
    ///
    /// Must be preceded by a call to [`Self::reset`].
    pub(super) fn snapshot(&self) -> Snapshot {
        let mut cache = self.inner.borrow_mut();
        cache.dirty = false;
        Snapshot::from_parts(
            self.parent_hash.expect("cache is reset before snapshotting"),
            cache.accounts.clone(),
            cache.storage.clone(),
            cache.bytecodes.clone(),
        )
    }

    /// Whether the cache holds reads not yet captured by [`Self::snapshot`].
    pub(super) fn is_dirty(&self) -> bool {
        self.inner.borrow().dirty
    }

    pub(super) const fn parent_hash(&self) -> Option<B256> {
        self.parent_hash
    }

    pub(super) fn state_provider(&self, inner: StateProviderBox) -> CacheStateProvider<'_> {
        CacheStateProvider { inner, cache: self }
    }

    fn get_or_try_insert_account_with<E>(
        &self,
        address: Address,
        f: impl FnOnce() -> Result<Option<Account>, E>,
    ) -> Result<Option<Account>, E> {
        if let Some(account) = self.inner.borrow().accounts.get(&address).copied() {
            return Ok(account)
        }

        let account = f()?;
        let mut cache = self.inner.borrow_mut();
        cache.accounts.insert(address, account);
        cache.dirty = true;
        Ok(account)
    }

    fn get_or_try_insert_storage_with<E>(
        &self,
        address: Address,
        key: StorageKey,
        f: impl FnOnce() -> Result<StorageValue, E>,
    ) -> Result<StorageValue, E> {
        if let Some(value) = self.inner.borrow().storage.get(&(address, key)).copied() {
            return Ok(value)
        }

        let value = f()?;
        let mut cache = self.inner.borrow_mut();
        cache.storage.insert((address, key), value);
        cache.dirty = true;
        Ok(value)
    }

    fn get_or_try_insert_code_with<E>(
        &self,
        code_hash: B256,
        f: impl FnOnce() -> Result<Option<Bytecode>, E>,
    ) -> Result<Option<Bytecode>, E> {
        if let Some(code) = self.inner.borrow().bytecodes.get(&code_hash).cloned() {
            return Ok(code)
        }

        let code = f()?;
        let mut cache = self.inner.borrow_mut();
        cache.bytecodes.insert(code_hash, code.clone());
        cache.dirty = true;
        Ok(code)
    }
}

/// Provider that fills only the reusable txpool-prewarm cache.
pub(super) struct CacheStateProvider<'a> {
    inner: StateProviderBox,
    cache: &'a Cache,
}

impl EvmStateProvider for CacheStateProvider<'_> {
    fn basic_account(
        &self,
        address: &Address,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Account>> {
        self.cache.get_or_try_insert_account_with(*address, || self.inner.basic_account(address))
    }

    fn block_hash(&self, number: BlockNumber) -> reth_errors::ProviderResult<Option<B256>> {
        EvmStateProvider::block_hash(&self.inner, number)
    }

    fn bytecode_by_hash(
        &self,
        code_hash: &B256,
    ) -> reth_errors::ProviderResult<Option<reth_primitives_traits::Bytecode>> {
        self.cache
            .get_or_try_insert_code_with(*code_hash, || self.inner.bytecode_by_hash(code_hash))
    }

    fn storage(
        &self,
        account: Address,
        storage_key: StorageKey,
    ) -> reth_errors::ProviderResult<Option<StorageValue>> {
        self.cache
            .get_or_try_insert_storage_with(account, storage_key, || {
                self.inner.storage(account, storage_key).map(Option::unwrap_or_default)
            })
            .map(|value| (!value.is_zero()).then_some(value))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dirty_tracks_inserts_not_hits() {
        let mut cache = Cache::default();
        cache.reset(B256::repeat_byte(0x01));
        assert!(!cache.is_dirty());

        cache.get_or_try_insert_account_with(Address::ZERO, || Ok::<_, ()>(None)).unwrap();
        assert!(cache.is_dirty());

        cache.snapshot();
        assert!(!cache.is_dirty());

        // A cache hit adds nothing worth republishing.
        cache.get_or_try_insert_account_with(Address::ZERO, || Ok::<_, ()>(None)).unwrap();
        assert!(!cache.is_dirty());

        cache
            .get_or_try_insert_account_with(Address::repeat_byte(0x02), || Ok::<_, ()>(None))
            .unwrap();
        cache.reset(B256::repeat_byte(0x02));
        assert!(!cache.is_dirty());
    }
}
