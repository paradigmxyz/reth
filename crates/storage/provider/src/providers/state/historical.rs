use crate::{
    providers::state::macros::delegate_provider_impls, AccountReader, BlockHashReader, PostState,
    ProviderError, StateProvider, StateRootProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    tables,
    transaction::DbTx,
};
use reth_interfaces::Result;
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, H256,
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
    /// Block number is main index for the history state of accounts and storages.
    block_number: BlockNumber,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

pub enum HistoryInfo {
    NotWritten,
    InChangeset(u64),
    InPlainState,
}

impl<'a, 'b, TX: DbTx<'a>> HistoricalStateProviderRef<'a, 'b, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: &'b TX, block_number: BlockNumber) -> Self {
        Self { tx, block_number, _phantom: PhantomData {} }
    }

    /// Lookup an account in the AccountHistory table
    pub fn account_history_lookup(&self, address: Address) -> Result<HistoryInfo> {
        // history key to search IntegerList of block number changesets.
        let history_key = ShardedKey::new(address, self.block_number);
        let mut cursor = self.tx.cursor_read::<tables::AccountHistory>()?;

        if let Some(chunk) =
            cursor.seek(history_key)?.filter(|(key, _)| key.key == address).map(|x| x.1 .0)
        {
            let chunk = chunk.enable_rank();
            let rank = chunk.rank(self.block_number as usize);
            if rank == 0 && !cursor.prev()?.is_some_and(|(key, _)| key.key == address) {
                return Ok(HistoryInfo::NotWritten)
            }
            if rank < chunk.len() {
                Ok(HistoryInfo::InChangeset(chunk.select(rank) as u64))
            } else {
                Ok(HistoryInfo::InPlainState)
            }
        } else {
            Ok(HistoryInfo::NotWritten)
        }
    }

    /// Lookup a storage key in the StorageHistory table
    pub fn storage_history_lookup(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> Result<HistoryInfo> {
        // history key to search IntegerList of block number changesets.
        let history_key = StorageShardedKey::new(address, storage_key, self.block_number);
        let mut cursor = self.tx.cursor_read::<tables::StorageHistory>()?;

        if let Some(chunk) = cursor
            .seek(history_key)?
            .filter(|(key, _)| key.address == address && key.sharded_key.key == storage_key)
            .map(|x| x.1 .0)
        {
            let chunk = chunk.enable_rank();
            let rank = chunk.rank(self.block_number as usize);
            if rank == 0 &&
                !cursor.prev()?.is_some_and(|(key, _)| {
                    key.address == address && key.sharded_key.key == storage_key
                })
            {
                return Ok(HistoryInfo::NotWritten)
            }
            if rank < chunk.len() {
                Ok(HistoryInfo::InChangeset(chunk.select(rank) as u64))
            } else {
                Ok(HistoryInfo::InPlainState)
            }
        } else {
            Ok(HistoryInfo::NotWritten)
        }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountReader for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> Result<Option<Account>> {
        match self.account_history_lookup(address)? {
            HistoryInfo::NotWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(self
                .tx
                .cursor_dup_read::<tables::AccountChangeSet>()?
                .seek_by_key_subkey(changeset_block_number, address)?
                .filter(|acc| acc.address == address)
                .ok_or(ProviderError::AccountChangesetNotFound {
                    block_number: changeset_block_number,
                    address,
                })?
                .info),
            HistoryInfo::InPlainState => Ok(self.tx.get::<tables::PlainAccountState>(address)?),
        }
    }
}

impl<'a, 'b, TX: DbTx<'a>> BlockHashReader for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> Result<Option<H256>> {
        self.tx.get::<tables::CanonicalHeaders>(number).map_err(Into::into)
    }

    fn canonical_hashes_range(&self, start: BlockNumber, end: BlockNumber) -> Result<Vec<H256>> {
        let range = start..end;
        self.tx
            .cursor_read::<tables::CanonicalHeaders>()
            .map(|mut cursor| {
                cursor
                    .walk_range(range)?
                    .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
                    .collect::<Result<Vec<_>>>()
            })?
            .map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateRootProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    fn state_root(&self, _post_state: PostState) -> Result<H256> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock.into())
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get storage.
    fn storage(&self, address: Address, storage_key: StorageKey) -> Result<Option<StorageValue>> {
        match self.storage_history_lookup(address, storage_key)? {
            HistoryInfo::NotWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(Some(
                self.tx
                    .cursor_dup_read::<tables::StorageChangeSet>()?
                    .seek_by_key_subkey((changeset_block_number, address).into(), storage_key)?
                    .filter(|entry| entry.key == storage_key)
                    .ok_or(ProviderError::StorageChangesetNotFound {
                        block_number: changeset_block_number,
                        address,
                        storage_key,
                    })?
                    .value,
            )),
            HistoryInfo::InPlainState => Ok(self
                .tx
                .cursor_dup_read::<tables::PlainStorageState>()?
                .seek_by_key_subkey(address, storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(|entry| entry.value)
                .or(Some(StorageValue::ZERO))),
        }
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: H256) -> Result<Option<Bytecode>> {
        self.tx.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }

    /// Get account and storage proofs.
    fn proof(
        &self,
        _address: Address,
        _keys: &[H256],
    ) -> Result<(Vec<Bytes>, H256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock.into())
    }
}

/// State provider for a given transition
pub struct HistoricalStateProvider<'a, TX: DbTx<'a>> {
    /// Database transaction
    tx: TX,
    /// State at the block number is the main indexer of the state.
    block_number: BlockNumber,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> HistoricalStateProvider<'a, TX> {
    /// Create new StateProvider from history transaction number
    pub fn new(tx: TX, block_number: BlockNumber) -> Self {
        Self { tx, block_number, _phantom: PhantomData {} }
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref<'b>(&'b self) -> HistoricalStateProviderRef<'a, 'b, TX> {
        HistoricalStateProviderRef::new(&self.tx, self.block_number)
    }
}

// Delegates all provider impls to [HistoricalStateProviderRef]
delegate_provider_impls!(HistoricalStateProvider<'a, TX> where [TX: DbTx<'a>]);

#[cfg(test)]
mod tests {
    use crate::{
        AccountReader, HistoricalStateProvider, HistoricalStateProviderRef, StateProvider,
    };
    use reth_db::{
        database::Database,
        models::{storage_sharded_key::StorageShardedKey, AccountBeforeTx, ShardedKey},
        tables,
        test_utils::create_test_rw_db,
        transaction::{DbTx, DbTxMut},
        BlockNumberList,
    };
    use reth_primitives::{hex_literal::hex, Account, StorageEntry, H160, H256, U256};

    const ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000001"));
    const HIGHER_ADDRESS: H160 = H160(hex!("0000000000000000000000000000000000000005"));
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
            ShardedKey { key: ADDRESS, highest_block_number: 7 },
            BlockNumberList::new([1, 3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::AccountHistory>(
            ShardedKey { key: ADDRESS, highest_block_number: u64::MAX },
            BlockNumberList::new([10, 15]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::AccountHistory>(
            ShardedKey { key: HIGHER_ADDRESS, highest_block_number: u64::MAX },
            BlockNumberList::new([4]).unwrap(),
        )
        .unwrap();

        let acc_plain = Account { nonce: 100, balance: U256::ZERO, bytecode_hash: None };
        let acc_at15 = Account { nonce: 15, balance: U256::ZERO, bytecode_hash: None };
        let acc_at10 = Account { nonce: 10, balance: U256::ZERO, bytecode_hash: None };
        let acc_at7 = Account { nonce: 7, balance: U256::ZERO, bytecode_hash: None };
        let acc_at3 = Account { nonce: 3, balance: U256::ZERO, bytecode_hash: None };

        let higher_acc_plain = Account { nonce: 4, balance: U256::ZERO, bytecode_hash: None };

        // setup
        tx.put::<tables::AccountChangeSet>(1, AccountBeforeTx { address: ADDRESS, info: None })
            .unwrap();
        tx.put::<tables::AccountChangeSet>(
            3,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at3) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSet>(
            4,
            AccountBeforeTx { address: HIGHER_ADDRESS, info: None },
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
        tx.put::<tables::PlainAccountState>(HIGHER_ADDRESS, higher_acc_plain).unwrap();
        tx.commit().unwrap();

        let tx = db.tx().unwrap();

        // run
        assert_eq!(HistoricalStateProviderRef::new(&tx, 1).basic_account(ADDRESS), Ok(None));
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 2).basic_account(ADDRESS),
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

        assert_eq!(HistoricalStateProviderRef::new(&tx, 1).basic_account(HIGHER_ADDRESS), Ok(None));
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1000).basic_account(HIGHER_ADDRESS),
            Ok(Some(higher_acc_plain))
        );
    }

    #[test]
    fn history_provider_get_storage() {
        let db = create_test_rw_db();
        let tx = db.tx_mut().unwrap();

        tx.put::<tables::StorageHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_block_number: 7 },
            },
            BlockNumberList::new([3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::StorageHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_block_number: u64::MAX },
            },
            BlockNumberList::new([10, 15]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::StorageHistory>(
            StorageShardedKey {
                address: HIGHER_ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_block_number: u64::MAX },
            },
            BlockNumberList::new([4]).unwrap(),
        )
        .unwrap();

        let higher_entry_plain = StorageEntry { key: STORAGE, value: U256::from(1000) };
        let higher_entry_at4 = StorageEntry { key: STORAGE, value: U256::from(0) };
        let entry_plain = StorageEntry { key: STORAGE, value: U256::from(100) };
        let entry_at15 = StorageEntry { key: STORAGE, value: U256::from(15) };
        let entry_at10 = StorageEntry { key: STORAGE, value: U256::from(10) };
        let entry_at7 = StorageEntry { key: STORAGE, value: U256::from(7) };
        let entry_at3 = StorageEntry { key: STORAGE, value: U256::from(0) };

        // setup
        tx.put::<tables::StorageChangeSet>((3, ADDRESS).into(), entry_at3).unwrap();
        tx.put::<tables::StorageChangeSet>((4, HIGHER_ADDRESS).into(), higher_entry_at4).unwrap();
        tx.put::<tables::StorageChangeSet>((7, ADDRESS).into(), entry_at7).unwrap();
        tx.put::<tables::StorageChangeSet>((10, ADDRESS).into(), entry_at10).unwrap();
        tx.put::<tables::StorageChangeSet>((15, ADDRESS).into(), entry_at15).unwrap();

        // setup plain state
        tx.put::<tables::PlainStorageState>(ADDRESS, entry_plain).unwrap();
        tx.put::<tables::PlainStorageState>(HIGHER_ADDRESS, higher_entry_plain).unwrap();
        tx.commit().unwrap();

        let tx = db.tx().unwrap();

        // run
        assert_eq!(HistoricalStateProviderRef::new(&tx, 0).storage(ADDRESS, STORAGE), Ok(None));
        assert_eq!(HistoricalStateProviderRef::new(&tx, 3).storage(ADDRESS, STORAGE), Ok(None));
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
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1).storage(HIGHER_ADDRESS, STORAGE),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1000).storage(HIGHER_ADDRESS, STORAGE),
            Ok(Some(higher_entry_plain.value))
        );
    }
}
