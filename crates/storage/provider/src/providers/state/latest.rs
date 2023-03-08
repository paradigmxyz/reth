use crate::{
    providers::state::macros::delegate_provider_impls, AccountProvider, BlockHashProvider,
    StateProvider,
};
use reth_db::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_interfaces::Result;
use reth_primitives::{Account, Address, Bytecode, StorageKey, StorageValue, H256, U256};
use std::marker::PhantomData;

/// State provider over latest state that takes tx reference.
pub struct LatestStateProviderRef<'a, 'b, TX: DbTx<'a>> {
    /// database transaction
    db: &'b TX,
    /// Phantom data over lifetime
    phantom: PhantomData<&'a TX>,
}

impl<'a, 'b, TX: DbTx<'a>> LatestStateProviderRef<'a, 'b, TX> {
    /// Create new state provider
    pub fn new(db: &'b TX) -> Self {
        Self { db, phantom: PhantomData {} }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        self.db.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> BlockHashProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        self.db.get::<tables::CanonicalHeaders>(number.to::<u64>()).map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for LatestStateProviderRef<'a, 'b, TX> {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let mut cursor = self.db.cursor_dup_read::<tables::PlainStorageState>()?;
        if let Some(entry) = cursor.seek_by_key_subkey(account, storage_key)? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        self.db.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

/// State provider for the latest state.
pub struct LatestStateProvider<'a, TX: DbTx<'a>> {
    /// database transaction
    db: TX,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> LatestStateProvider<'a, TX> {
    /// Create new state provider
    pub fn new(db: TX) -> Self {
        Self { db, _phantom: PhantomData {} }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref<'b>(&'b self) -> LatestStateProviderRef<'a, 'b, TX> {
        LatestStateProviderRef::new(&self.db)
    }
}

// Delegates all provider impls to [LatestStateProviderRef]
delegate_provider_impls!(LatestStateProvider<'a, TX> where [TX: DbTx<'a>]);

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(unused)]
    fn assert_latest_state_provider<'txn, T: DbTx<'txn> + 'txn>() {
        assert_state_provider::<LatestStateProvider<'txn, T>>();
    }
}
