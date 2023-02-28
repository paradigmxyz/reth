use crate::{
    providers::state::macros::delegate_provider_impls, AccountProvider, BlockHashProvider,
    ProviderError, StateProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
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
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        // history key to search IntegerList of transition id changesets.
        let history_key = ShardedKey::new(address, self.transition);

        let Some(changeset_transition_id) = self.tx.cursor_read::<tables::AccountHistory>()?
            .seek(history_key)?
            .filter(|(key,_)| key.key == address)
            .map(|(_,list)| list.0.enable_rank().successor(self.transition as usize).map(|i| i as u64)) else {
                return Ok(None)
            };

        // if changeset transition id is present we are getting value from changeset
        if let Some(changeset_transition_id) = changeset_transition_id {
            let account = self
                .tx
                .cursor_dup_read::<tables::AccountChangeSet>()?
                .seek_by_key_subkey(changeset_transition_id, address)?
                .filter(|acc| acc.address == address)
                .ok_or(ProviderError::AccountChangeset {
                    transition_id: changeset_transition_id,
                    address,
                })?;
            Ok(account.info)
        } else {
            // if changeset is not present that means that there was history shard but we need to
            // use newest value from plain state
            Ok(self.tx.get::<tables::PlainAccountState>(address)?)
        }
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
    fn storage(&self, address: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        // history key to search IntegerList of transition id changesets.
        let history_key = StorageShardedKey::new(address, storage_key, self.transition);

        let Some(changeset_transition_id) = self.tx.cursor_read::<tables::StorageHistory>()?
            .seek(history_key)?
            .filter(|(key,_)| key.address == address && key.sharded_key.key == storage_key)
            .map(|(_,list)| list.0.enable_rank().successor(self.transition as usize).map(|i| i as u64)) else {
                return Ok(None)
            };

        // if changeset transition id is present we are getting value from changeset
        if let Some(changeset_transition_id) = changeset_transition_id {
            let storage_entry = self
                .tx
                .cursor_dup_read::<tables::StorageChangeSet>()?
                .seek_by_key_subkey((changeset_transition_id, address).into(), storage_key)?
                .filter(|entry| entry.key == storage_key)
                .ok_or(ProviderError::StorageChangeset {
                    transition_id: changeset_transition_id,
                    address,
                    storage_key,
                })?;
            Ok(Some(storage_entry.value))
        } else {
            // if changeset is not present that means that there was history shard but we need to
            // use newest value from plain state
            Ok(self
                .tx
                .cursor_dup_read::<tables::PlainStorageState>()?
                .seek_by_key_subkey(address, storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(|entry| entry.value))
        }
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

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref<'b>(&'b self) -> HistoricalStateProviderRef<'a, 'b, TX> {
        HistoricalStateProviderRef::new(&self.tx, self.transition)
    }
}

// Delegates all provider impls to [HistoricalStateProviderRef]
delegate_provider_impls!(HistoricalStateProvider<'a, TX> where [TX: DbTx<'a>]);

#[cfg(test)]
mod tests {
    use crate::{
        AccountProvider, HistoricalStateProvider, HistoricalStateProviderRef, StateProvider,
    };
    use reth_db::{
        database::Database,
        mdbx::test_utils::create_test_rw_db,
        models::{storage_sharded_key::StorageShardedKey, AccountBeforeTx, ShardedKey},
        tables,
        transaction::{DbTx, DbTxMut},
        TransitionList,
    };
    use reth_primitives::{hex_literal::hex, Account, StorageEntry, H160, H256, U256};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));
    const STORAGE: H256 =
        H256(hex!("0000000000000000000000000000000000000000000000000000000000000001"));

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(unused)]
    fn assert_historical_state_provider<'txn, T: DbTx<'txn> + 'txn>() {
        assert_state_provider::<HistoricalStateProvider<'txn, T>>();
    }

    #[test]
    fn history_provider_get_account() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();

        tx.put::<tables::AccountHistory>(
            ShardedKey { key: ADDRESS, highest_transition_id: 7 },
            TransitionList::new([3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::AccountHistory>(
            ShardedKey { key: ADDRESS, highest_transition_id: u64::MAX },
            TransitionList::new([10, 15]).unwrap(),
        )
        .unwrap();

        let acc_plain = Account { nonce: 100, balance: U256::ZERO, bytecode_hash: None };
        let acc_at15 = Account { nonce: 15, balance: U256::ZERO, bytecode_hash: None };
        let acc_at10 = Account { nonce: 10, balance: U256::ZERO, bytecode_hash: None };
        let acc_at7 = Account { nonce: 7, balance: U256::ZERO, bytecode_hash: None };
        let acc_at3 = Account { nonce: 3, balance: U256::ZERO, bytecode_hash: None };

        // setup
        tx.put::<tables::AccountChangeSet>(
            3,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at3) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSet>(
            7,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at7) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSet>(
            10,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at10) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSet>(
            15,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at15) },
        )
        .unwrap();

        // setup plain state
        tx.put::<tables::PlainAccountState>(ADDRESS, acc_plain).unwrap();
        tx.commit().unwrap();

        let tx = db.tx().unwrap();

        // run
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1).basic_account(ADDRESS),
            Ok(Some(acc_at3))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 3).basic_account(ADDRESS),
            Ok(Some(acc_at3))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 4).basic_account(ADDRESS),
            Ok(Some(acc_at7))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 7).basic_account(ADDRESS),
            Ok(Some(acc_at7))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 9).basic_account(ADDRESS),
            Ok(Some(acc_at10))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 10).basic_account(ADDRESS),
            Ok(Some(acc_at10))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 11).basic_account(ADDRESS),
            Ok(Some(acc_at15))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 16).basic_account(ADDRESS),
            Ok(Some(acc_plain))
        );
    }

    #[test]
    fn history_provider_get_storage() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();

        tx.put::<tables::StorageHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_transition_id: 7 },
            },
            TransitionList::new([3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::StorageHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_transition_id: u64::MAX },
            },
            TransitionList::new([10, 15]).unwrap(),
        )
        .unwrap();

        let entry_plain = StorageEntry { key: STORAGE, value: U256::from(100) };
        let entry_at15 = StorageEntry { key: STORAGE, value: U256::from(15) };
        let entry_at10 = StorageEntry { key: STORAGE, value: U256::from(10) };
        let entry_at7 = StorageEntry { key: STORAGE, value: U256::from(7) };
        let entry_at3 = StorageEntry { key: STORAGE, value: U256::from(0) };

        // setup
        tx.put::<tables::StorageChangeSet>((3, ADDRESS).into(), entry_at3).unwrap();
        tx.put::<tables::StorageChangeSet>((7, ADDRESS).into(), entry_at7).unwrap();
        tx.put::<tables::StorageChangeSet>((10, ADDRESS).into(), entry_at10).unwrap();
        tx.put::<tables::StorageChangeSet>((15, ADDRESS).into(), entry_at15).unwrap();

        // setup plain state
        tx.put::<tables::PlainStorageState>(ADDRESS, entry_plain).unwrap();
        tx.commit().unwrap();

        let tx = db.tx().unwrap();

        // run
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 0).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at3.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 3).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at3.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 4).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at7.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 7).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at7.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 9).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at10.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 10).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at10.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 11).storage(ADDRESS, STORAGE),
            Ok(Some(entry_at15.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 16).storage(ADDRESS, STORAGE),
            Ok(Some(entry_plain.value))
        );
    }
}
