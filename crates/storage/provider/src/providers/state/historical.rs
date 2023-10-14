use crate::{
    providers::state::macros::delegate_provider_impls, AccountReader, BlockHashReader,
    BundleStateWithReceipts, ProviderError, StateProvider, StateRootProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::Table,
    tables,
    transaction::DbTx,
    BlockNumberList,
};
use reth_interfaces::RethResult;
use reth_primitives::{
    Account, Address, BlockNumber, Bytecode, Bytes, StorageKey, StorageValue, B256,
};
use std::marker::PhantomData;

/// State provider for a given block number which takes a tx reference.
///
/// Historical state provider accesses the state at the start of the provided block number.
/// It means that all changes made in the provided block number are not included.
///
/// Historical state provider reads the following tables:
/// - [tables::AccountHistory]
/// - [tables::Bytecodes]
/// - [tables::StorageHistory]
/// - [tables::AccountChangeSet]
/// - [tables::StorageChangeSet]
#[derive(Debug)]
pub struct HistoricalStateProviderRef<'a, 'b, TX: DbTx<'a>> {
    /// Transaction
    tx: &'b TX,
    /// Block number is main index for the history state of accounts and storages.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    NotYetWritten,
    InChangeset(u64),
    InPlainState,
    MaybeInPlainState,
}

impl<'a, 'b, TX: DbTx<'a>> HistoricalStateProviderRef<'a, 'b, TX> {
    /// Create new StateProvider for historical block number
    pub fn new(tx: &'b TX, block_number: BlockNumber) -> Self {
        Self {
            tx,
            block_number,
            lowest_available_blocks: Default::default(),
            _phantom: PhantomData {},
        }
    }

    /// Create new StateProvider for historical block number and lowest block numbers at which
    /// account & storage histories are available.
    pub fn new_with_lowest_available_blocks(
        tx: &'b TX,
        block_number: BlockNumber,
        lowest_available_blocks: LowestAvailableBlocks,
    ) -> Self {
        Self { tx, block_number, lowest_available_blocks, _phantom: PhantomData {} }
    }

    /// Lookup an account in the AccountHistory table
    pub fn account_history_lookup(&self, address: Address) -> RethResult<HistoryInfo> {
        if !self.lowest_available_blocks.is_account_history_available(self.block_number) {
            return Err(ProviderError::StateAtBlockPruned(self.block_number).into())
        }

        // history key to search IntegerList of block number changesets.
        let history_key = ShardedKey::new(address, self.block_number);
        self.history_info::<tables::AccountHistory, _>(
            history_key,
            |key| key.key == address,
            self.lowest_available_blocks.account_history_block_number,
        )
    }

    /// Lookup a storage key in the StorageHistory table
    pub fn storage_history_lookup(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> RethResult<HistoryInfo> {
        if !self.lowest_available_blocks.is_storage_history_available(self.block_number) {
            return Err(ProviderError::StateAtBlockPruned(self.block_number).into())
        }

        // history key to search IntegerList of block number changesets.
        let history_key = StorageShardedKey::new(address, storage_key, self.block_number);
        self.history_info::<tables::StorageHistory, _>(
            history_key,
            |key| key.address == address && key.sharded_key.key == storage_key,
            self.lowest_available_blocks.storage_history_block_number,
        )
    }

    fn history_info<T, K>(
        &self,
        key: K,
        key_filter: impl Fn(&K) -> bool,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> RethResult<HistoryInfo>
    where
        T: Table<Key = K, Value = BlockNumberList>,
    {
        let mut cursor = self.tx.cursor_read::<T>()?;

        // Lookup the history chunk in the history index. If they key does not appear in the
        // index, the first chunk for the next key will be returned so we filter out chunks that
        // have a different key.
        if let Some(chunk) = cursor.seek(key)?.filter(|(key, _)| key_filter(key)).map(|x| x.1 .0) {
            let chunk = chunk.enable_rank();

            // Get the rank of the first entry after our block.
            let rank = chunk.rank(self.block_number as usize);

            // If our block is before the first entry in the index chunk and this first entry
            // doesn't equal to our block, it might be before the first write ever. To check, we
            // look at the previous entry and check if the key is the same.
            // This check is worth it, the `cursor.prev()` check is rarely triggered (the if will
            // short-circuit) and when it passes we save a full seek into the changeset/plain state
            // table.
            if rank == 0 &&
                chunk.select(rank) as u64 != self.block_number &&
                !cursor.prev()?.is_some_and(|(key, _)| key_filter(&key))
            {
                if lowest_available_block_number.is_some() {
                    // The key may have been written, but due to pruning we may not have changesets
                    // and history, so we need to make a changeset lookup.
                    Ok(HistoryInfo::InChangeset(chunk.select(rank) as u64))
                } else {
                    // The key is written to, but only after our block.
                    Ok(HistoryInfo::NotYetWritten)
                }
            } else if rank < chunk.len() {
                // The chunk contains an entry for a write after our block, return it.
                Ok(HistoryInfo::InChangeset(chunk.select(rank) as u64))
            } else {
                // The chunk does not contain an entry for a write after our block. This can only
                // happen if this is the last chunk and so we need to look in the plain state.
                Ok(HistoryInfo::InPlainState)
            }
        } else if lowest_available_block_number.is_some() {
            // The key may have been written, but due to pruning we may not have changesets and
            // history, so we need to make a plain state lookup.
            Ok(HistoryInfo::MaybeInPlainState)
        } else {
            // The key has not been written to at all.
            Ok(HistoryInfo::NotYetWritten)
        }
    }
}

impl<'a, 'b, TX: DbTx<'a>> AccountReader for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> RethResult<Option<Account>> {
        match self.account_history_lookup(address)? {
            HistoryInfo::NotYetWritten => Ok(None),
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
            HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => {
                Ok(self.tx.get::<tables::PlainAccountState>(address)?)
            }
        }
    }
}

impl<'a, 'b, TX: DbTx<'a>> BlockHashReader for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> RethResult<Option<B256>> {
        self.tx.get::<tables::CanonicalHeaders>(number).map_err(Into::into)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> RethResult<Vec<B256>> {
        let range = start..end;
        self.tx
            .cursor_read::<tables::CanonicalHeaders>()
            .map(|mut cursor| {
                cursor
                    .walk_range(range)?
                    .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
                    .collect::<RethResult<Vec<_>>>()
            })?
            .map_err(Into::into)
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateRootProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    fn state_root(&self, _post_state: &BundleStateWithReceipts) -> RethResult<B256> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock.into())
    }
}

impl<'a, 'b, TX: DbTx<'a>> StateProvider for HistoricalStateProviderRef<'a, 'b, TX> {
    /// Get storage.
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> RethResult<Option<StorageValue>> {
        match self.storage_history_lookup(address, storage_key)? {
            HistoryInfo::NotYetWritten => Ok(None),
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
            HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => Ok(self
                .tx
                .cursor_dup_read::<tables::PlainStorageState>()?
                .seek_by_key_subkey(address, storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(|entry| entry.value)
                .or(Some(StorageValue::ZERO))),
        }
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: B256) -> RethResult<Option<Bytecode>> {
        self.tx.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }

    /// Get account and storage proofs.
    fn proof(
        &self,
        _address: Address,
        _keys: &[B256],
    ) -> RethResult<(Vec<Bytes>, B256, Vec<Vec<Bytes>>)> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock.into())
    }
}

/// State provider for a given block number.
/// For more detailed description, see [HistoricalStateProviderRef].
#[derive(Debug)]
pub struct HistoricalStateProvider<'a, TX: DbTx<'a>> {
    /// Database transaction
    tx: TX,
    /// State at the block number is the main indexer of the state.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
    /// Phantom lifetime `'a`
    _phantom: PhantomData<&'a TX>,
}

impl<'a, TX: DbTx<'a>> HistoricalStateProvider<'a, TX> {
    /// Create new StateProvider for historical block number
    pub fn new(tx: TX, block_number: BlockNumber) -> Self {
        Self {
            tx,
            block_number,
            lowest_available_blocks: Default::default(),
            _phantom: PhantomData {},
        }
    }

    /// Set the lowest block number at which the account history is available.
    pub fn with_lowest_available_account_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.account_history_block_number = Some(block_number);
        self
    }

    /// Set the lowest block number at which the storage history is available.
    pub fn with_lowest_available_storage_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.storage_history_block_number = Some(block_number);
        self
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    fn as_ref<'b>(&'b self) -> HistoricalStateProviderRef<'a, 'b, TX> {
        HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &self.tx,
            self.block_number,
            self.lowest_available_blocks,
        )
    }
}

// Delegates all provider impls to [HistoricalStateProviderRef]
delegate_provider_impls!(HistoricalStateProvider<'a, TX> where [TX: DbTx<'a>]);

/// Lowest blocks at which different parts of the state are available.
/// They may be [Some] if pruning is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowestAvailableBlocks {
    /// Lowest block number at which the account history is available. It may not be available if
    /// [reth_primitives::PruneSegment::AccountHistory] was pruned.
    /// [Option::None] means all history is available.
    pub account_history_block_number: Option<BlockNumber>,
    /// Lowest block number at which the storage history is available. It may not be available if
    /// [reth_primitives::PruneSegment::StorageHistory] was pruned.
    /// [Option::None] means all history is available.
    pub storage_history_block_number: Option<BlockNumber>,
}

impl LowestAvailableBlocks {
    /// Check if account history is available at the provided block number, i.e. lowest available
    /// block number for account history is less than or equal to the provided block number.
    pub fn is_account_history_available(&self, at: BlockNumber) -> bool {
        self.account_history_block_number.map(|block_number| block_number <= at).unwrap_or(true)
    }

    /// Check if storage history is available at the provided block number, i.e. lowest available
    /// block number for storage history is less than or equal to the provided block number.
    pub fn is_storage_history_available(&self, at: BlockNumber) -> bool {
        self.storage_history_block_number.map(|block_number| block_number <= at).unwrap_or(true)
    }
}

#[cfg(test)]
mod tests {
    use crate::{
        providers::state::historical::{HistoryInfo, LowestAvailableBlocks},
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
    use reth_interfaces::provider::ProviderError;
    use reth_primitives::{address, b256, Account, Address, StorageEntry, B256, U256};

    const ADDRESS: Address = address!("0000000000000000000000000000000000000001");
    const HIGHER_ADDRESS: Address = address!("0000000000000000000000000000000000000005");
    const STORAGE: B256 = b256!("0000000000000000000000000000000000000000000000000000000000000001");

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
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 3).storage(ADDRESS, STORAGE),
            Ok(Some(U256::ZERO))
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
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1).storage(HIGHER_ADDRESS, STORAGE),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1000).storage(HIGHER_ADDRESS, STORAGE),
            Ok(Some(higher_entry_plain.value))
        );
    }

    #[test]
    fn history_provider_unavailable() {
        let db = create_test_rw_db();
        let tx = db.tx().unwrap();

        // provider block_number < lowest available block number,
        // i.e. state at provider block is pruned
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &tx,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(3),
                storage_history_block_number: Some(3),
            },
        );
        assert_eq!(
            provider.account_history_lookup(ADDRESS),
            Err(ProviderError::StateAtBlockPruned(provider.block_number).into())
        );
        assert_eq!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Err(ProviderError::StateAtBlockPruned(provider.block_number).into())
        );

        // provider block_number == lowest available block number,
        // i.e. state at provider block is available
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &tx,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(2),
                storage_history_block_number: Some(2),
            },
        );
        assert_eq!(provider.account_history_lookup(ADDRESS), Ok(HistoryInfo::MaybeInPlainState));
        assert_eq!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Ok(HistoryInfo::MaybeInPlainState)
        );

        // provider block_number == lowest available block number,
        // i.e. state at provider block is available
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &tx,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(1),
                storage_history_block_number: Some(1),
            },
        );
        assert_eq!(provider.account_history_lookup(ADDRESS), Ok(HistoryInfo::MaybeInPlainState));
        assert_eq!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Ok(HistoryInfo::MaybeInPlainState)
        );
    }
}
