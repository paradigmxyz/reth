use super::ProviderImpl;
use crate::{AccountProvider, Error, StateProvider, StateProviderFactory};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    database::{Database, DatabaseGAT},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;

use reth_primitives::{
    Account, Address, BlockHash, BlockNumber, Bytes, StorageKey, StorageValue, TransitionId, H256,
    U256,
};
use std::marker::PhantomData;

impl<DB: Database> StateProviderFactory for ProviderImpl<DB> {
    type HistorySP<'a> = StateProviderImplHistory<'a,<DB as DatabaseGAT<'a>>::TX> where Self: 'a;
    type LatestSP<'a> = StateProviderImplLatest<'a,<DB as DatabaseGAT<'a>>::TX> where Self: 'a;
    /// Storage provider for latest block
    fn latest(&self) -> Result<Self::LatestSP<'_>> {
        Ok(StateProviderImplLatest::new(self.db.tx()?))
    }

    fn history_by_block_number(&self, block_number: BlockNumber) -> Result<Self::HistorySP<'_>> {
        let tx = self.db.tx()?;
        // get block hash
        let block_hash = tx
            .get::<tables::CanonicalHeaders>(block_number)?
            .ok_or(Error::BlockNumber { block_number })?;

        // get transition id
        let block_num_hash = (block_number, block_hash);
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_num_hash.into())?
            .ok_or(Error::BlockTransition { block_number, block_hash })?;

        Ok(StateProviderImplHistory::new(tx, transition))
    }

    fn history_by_block_hash(&self, block_hash: BlockHash) -> Result<Self::HistorySP<'_>> {
        let tx = self.db.tx()?;
        // get block number
        let block_number =
            tx.get::<tables::HeaderNumbers>(block_hash)?.ok_or(Error::BlockHash { block_hash })?;

        // get transition id
        let block_num_hash = (block_number, block_hash);
        let transition = tx
            .get::<tables::BlockTransitionIndex>(block_num_hash.into())?
            .ok_or(Error::BlockTransition { block_number, block_hash })?;

        Ok(StateProviderImplHistory::new(tx, transition))
    }
}

/// State provider for a given transition
pub struct StateProviderImplHistory<'a, TX: DbTx<'a>> {
    /// Database transaction
    tx: TX,
    /// Transition is main indexer of account and storage changes
    transition: TransitionId,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> StateProviderImplHistory<'a, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: TX, transition: TransitionId) -> Self {
        Self { tx, transition, _phantom: PhantomData {} }
    }
}

impl<'a, TX: DbTx<'a>> AccountProvider for StateProviderImplHistory<'a, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        StateProviderImplRefHistory::new(&self.tx, self.transition).basic_account(address)
    }
}

impl<'a, TX: DbTx<'a>> StateProvider for StateProviderImplHistory<'a, TX> {
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        StateProviderImplRefHistory::new(&self.tx, self.transition).storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>> {
        StateProviderImplRefHistory::new(&self.tx, self.transition).bytecode_by_hash(code_hash)
    }

    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        StateProviderImplRefHistory::new(&self.tx, self.transition).block_hash(number)
    }
}
/// State provider with given hash
///
/// It will access:
/// [tables::AccountHistory]
/// [tables::Bytecodes]
/// [tables::StorageHistory]
/// [tables::AccountChangeSet]
/// [tables::StorageChangeSet]
pub struct StateProviderImplRefHistory<'a, 'b, TX: DbTx<'a>> {
    /// Transaction
    tx: &'b TX,
    /// Transition is main indexer of account and storage changes
    transition: TransitionId,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, 'b, TX: DbTx<'a>> StateProviderImplRefHistory<'a, 'b, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: &'b TX, transition: TransitionId) -> Self {
        Self { tx, transition, _phantom: PhantomData {} }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountProvider for StateProviderImplRefHistory<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, _address: Address) -> Result<Option<Account>> {
        // TODO add when AccountHistory is defined
        Ok(None)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for StateProviderImplRefHistory<'a, 'b, TX> {
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
        let mut cursor = self.tx.cursor_dup::<tables::StorageChangeSet>()?;

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

    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        self.tx.get::<tables::CanonicalHeaders>(number.as_u64()).map_err(Into::into)
    }
}

/// State provider for latests state
pub struct StateProviderImplLatest<'a, TX: DbTx<'a>> {
    /// database transaction
    db: TX,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> StateProviderImplLatest<'a, TX> {
    /// Create new state provider
    pub fn new(db: TX) -> Self {
        Self { db, _phantom: PhantomData {} }
    }
}

impl<'a, TX: DbTx<'a>> AccountProvider for StateProviderImplLatest<'a, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        StateProviderImplRefLatest::new(&self.db).basic_account(address)
    }
}

impl<'a, TX: DbTx<'a>> StateProvider for StateProviderImplLatest<'a, TX> {
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        StateProviderImplRefLatest::new(&self.db).storage(account, storage_key)
    }

    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytes>> {
        StateProviderImplRefLatest::new(&self.db).bytecode_by_hash(code_hash)
    }

    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        StateProviderImplRefLatest::new(&self.db).block_hash(number)
    }
}

/// State Provider over latest state that takes tx reference
pub struct StateProviderImplRefLatest<'a, 'b, TX: DbTx<'a>> {
    /// database transaction
    db: &'b TX,
    /// Phantom data over lifetime
    phantom: PhantomData<&'a TX>,
}

impl<'a, 'b, TX: DbTx<'a>> StateProviderImplRefLatest<'a, 'b, TX> {
    /// Create new state provider
    pub fn new(db: &'b TX) -> Self {
        Self { db, phantom: PhantomData {} }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountProvider for StateProviderImplRefLatest<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        self.db.get::<tables::PlainAccountState>(address).map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for StateProviderImplRefLatest<'a, 'b, TX> {
    /// Get storage.
    fn storage(&self, account: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        let mut cursor = self.db.cursor_dup::<tables::PlainStorageState>()?;
        if let Some((_, entry)) = cursor.seek_exact(account)? {
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
        self.db.get::<tables::Bytecodes>(code_hash).map_err(Into::into).map(|r| r.map(Bytes::from))
    }

    /// Get block hash by number.
    fn block_hash(&self, number: U256) -> Result<Option<H256>> {
        self.db.get::<tables::CanonicalHeaders>(number.as_u64()).map_err(Into::into)
    }
}
