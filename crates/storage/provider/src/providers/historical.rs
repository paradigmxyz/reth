use crate::{AccountProvider, BlockHashProvider, StateProvider};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;
use reth_primitives::{
    Account, Address, Bytes, StorageKey, StorageValue, TransitionId, H256, U256,
};
use std::marker::PhantomData;

/// State provider for a given transition id which takes a tx reference.
///
/// Historical state provider reads the following tables:
/// [tables::AccountHistory]
/// [tables::Bytecodes]
/// [tables::StorageHistory]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
pub struct HistoricalStateProviderRef<'a, 'b, TX: DbTx<'a>> {
    /// Transaction
    tx: &'b TX,
    /// Transition is main indexer of account and storage changes
    transition: TransitionId,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, 'b, TX: DbTx<'a>> HistoricalStateProviderRef<'a, 'b, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: &'b TX, transition: TransitionId) -> Self {
        Self { tx, transition, _phantom: PhantomData {} }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
        // TODO add when AccountHistory is defined
        Ok(None)
    }
}

impl<'a, 'b, TX: DbTx<'a>> BlockHashProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        self.tx.get::<tables::CanonicalHeaders>(number.to::<u64>()).map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        // TODO when StorageHistory is defined
        let transaction_number =
            self.tx.get::<tables::StorageHistory>(Vec::new())?.map(|_integer_list|
            // TODO select integer that is one less from transaction_number <- // TODO: (rkrasiuk) not sure this comment is still relevant
            self.transition);

        if transaction_number.is_none() {
            return Ok(None)
        }
        let num = transaction_number.unwrap();
        let mut cursor = self.tx.cursor_dup_read::<tables::StorageChangeSet>()?;

        if let Some((_, entry)) = cursor.seek_exact((num, account).into())? {
            if entry.key == storage_key {
                return Ok(Some(entry.value))
            }

            if let Some((_, entry)) = cursor.seek(storage_key)? {
                if entry.key == storage_key {
                    return Ok(Some(entry.value))
                }
            }
        }
        Ok(None)
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>> {
        self.tx.get::<tables::Bytecodes>(code_hash).map_err(Into::into).map(|r| r.map(Bytes::from))
    }
}

/// State provider for a given transition
pub struct HistoricalStateProvider<'a, TX: DbTx<'a>> {
    /// Database transaction
    tx: TX,
    /// Transition is main indexer of account and storage changes
    transition: TransitionId,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> HistoricalStateProvider<'a, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: TX, transition: TransitionId) -> Self {
        Self { tx, transition, _phantom: PhantomData {} }
    }
}

/// Derive trait implementation for [HistoricalStateProvider]
/// from [HistoricalStateProviderRef] type.
///
/// Used to implement provider traits.
macro_rules! derive_from_ref {
    ($trait:ident, $(fn $func:ident(&self$(, )?$($arg_name:ident: $arg:ty),*) -> $ret:ty),*) => {
        impl<'a, TX: DbTx<'a>> $trait for HistoricalStateProvider<'a, TX> {
            $(fn $func(&self, $($arg_name: $arg),*) -> $ret {
                HistoricalStateProviderRef::new(&self.tx, self.transition).$func($($arg_name),*)
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
