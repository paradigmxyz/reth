use crate::{AccountProvider, BlockHashProvider, StateProvider};
use reth_db::{cursor::DbDupCursorRO, tables, transaction::DbTx};
use reth_interfaces::Result;
use reth_primitives::{Account, Address, Bytes, StorageKey, StorageValue, H256, U256};
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
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>> {
        self.db.get::<tables::Bytecodes>(code_hash).map_err(Into::into).map(|r| r.map(Bytes::from))
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
}

/// Derive trait implementation for [LatestStateProvider]
/// from [LatestStateProviderRef] type.
///
/// Used to implement provider traits.
macro_rules! derive_from_ref {
    ($trait:ident, $(fn $func:ident(&self$(, )?$($arg_name:ident: $arg:ty),*) -> $ret:ty),*) => {
        impl<'a, TX: DbTx<'a>> $trait for LatestStateProvider<'a, TX> {
            $(fn $func(&self, $($arg_name: $arg),*) -> $ret {
                LatestStateProviderRef::new(&self.db).$func($($arg_name),*)
            })*
        }
    };
}

derive_from_ref!(AccountProvider, fn basic_account(&self, address: Address) -> Result<Option<Account>>);
derive_from_ref!(BlockHashProvider, fn block_hash(&self, number: U256) -> Result<Option<H256>>);
derive_from_ref!(
    StateProvider,
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>>,
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>>
);
