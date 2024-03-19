use crate::{
    providers::{state::macros::delegate_provider_impls, StaticFileProvider},
    AccountReader, BlockHashReader, ProviderError, StateProvider, StateRootProvider,
};
use reth_db::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::Table,
    tables,
    transaction::DbTx,
    BlockNumberList,
};
use reth_interfaces::provider::ProviderResult;
use reth_primitives::{
    constants::EPOCH_SLOTS, trie::AccountProof, Account, Address, BlockNumber, Bytecode,
    StaticFileSegment, StorageKey, StorageValue, B256,
};
use reth_trie::{updates::TrieUpdates, HashedPostState};
use revm::db::BundleState;
use std::fmt::Debug;

/// State provider for a given block number which takes a tx reference.
///
/// Historical state provider accesses the state at the start of the provided block number.
/// It means that all changes made in the provided block number are not included.
///
/// Historical state provider reads the following tables:
/// - [tables::AccountsHistory]
/// - [tables::Bytecodes]
/// - [tables::StoragesHistory]
/// - [tables::AccountChangeSets]
/// - [tables::StorageChangeSets]
#[derive(Debug)]
pub struct HistoricalStateProviderRef<'b, TX: DbTx> {
    /// Transaction
    tx: &'b TX,
    /// Block number is main index for the history state of accounts and storages.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
    /// Static File provider
    static_file_provider: StaticFileProvider,
}

#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    NotYetWritten,
    InChangeset(u64),
    InPlainState,
    MaybeInPlainState,
}

impl<'b, TX: DbTx> HistoricalStateProviderRef<'b, TX> {
    /// Create new StateProvider for historical block number
    pub fn new(
        tx: &'b TX,
        block_number: BlockNumber,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        Self { tx, block_number, lowest_available_blocks: Default::default(), static_file_provider }
    }

    /// Create new StateProvider for historical block number and lowest block numbers at which
    /// account & storage histories are available.
    pub fn new_with_lowest_available_blocks(
        tx: &'b TX,
        block_number: BlockNumber,
        lowest_available_blocks: LowestAvailableBlocks,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        Self { tx, block_number, lowest_available_blocks, static_file_provider }
    }

    /// Lookup an account in the AccountsHistory table
    pub fn account_history_lookup(&self, address: Address) -> ProviderResult<HistoryInfo> {
        if !self.lowest_available_blocks.is_account_history_available(self.block_number) {
            return Err(ProviderError::StateAtBlockPruned(self.block_number))
        }

        // history key to search IntegerList of block number changesets.
        let history_key = ShardedKey::new(address, self.block_number);
        self.history_info::<tables::AccountsHistory, _>(
            history_key,
            |key| key.key == address,
            self.lowest_available_blocks.account_history_block_number,
        )
    }

    /// Lookup a storage key in the StoragesHistory table
    pub fn storage_history_lookup(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<HistoryInfo> {
        if !self.lowest_available_blocks.is_storage_history_available(self.block_number) {
            return Err(ProviderError::StateAtBlockPruned(self.block_number))
        }

        // history key to search IntegerList of block number changesets.
        let history_key = StorageShardedKey::new(address, storage_key, self.block_number);
        self.history_info::<tables::StoragesHistory, _>(
            history_key,
            |key| key.address == address && key.sharded_key.key == storage_key,
            self.lowest_available_blocks.storage_history_block_number,
        )
    }

    /// Retrieve revert hashed state for this history provider.
    fn revert_state(&self) -> ProviderResult<HashedPostState> {
        if !self.lowest_available_blocks.is_account_history_available(self.block_number) ||
            !self.lowest_available_blocks.is_storage_history_available(self.block_number)
        {
            return Err(ProviderError::StateAtBlockPruned(self.block_number))
        }

        let tip = self
            .tx
            .cursor_read::<tables::CanonicalHeaders>()?
            .last()?
            .map(|(tip, _)| tip)
            .or_else(|| {
                self.static_file_provider.get_highest_static_file_block(StaticFileSegment::Headers)
            })
            .ok_or(ProviderError::BestBlockNotFound)?;

        if tip.saturating_sub(self.block_number) > EPOCH_SLOTS {
            tracing::warn!(
                target: "provider::historical_sp",
                target = self.block_number,
                "Attempt to calculate state root for an old block might result in OOM, tread carefully"
            );
        }

        Ok(HashedPostState::from_revert_range(self.tx, self.block_number..=tip)?)
    }

    fn history_info<T, K>(
        &self,
        key: K,
        key_filter: impl Fn(&K) -> bool,
        lowest_available_block_number: Option<BlockNumber>,
    ) -> ProviderResult<HistoryInfo>
    where
        T: Table<Key = K, Value = BlockNumberList>,
    {
        let mut cursor = self.tx.cursor_read::<T>()?;

        // Lookup the history chunk in the history index. If they key does not appear in the
        // index, the first chunk for the next key will be returned so we filter out chunks that
        // have a different key.
        if let Some(chunk) = cursor.seek(key)?.filter(|(key, _)| key_filter(key)).map(|x| x.1 .0) {
            // Get the rank of the first entry before or equal to our block.
            let mut rank = chunk.rank(self.block_number);

            // Adjust the rank, so that we have the rank of the first entry strictly before our
            // block (not equal to it).
            if rank.checked_sub(1).and_then(|rank| chunk.select(rank)) == Some(self.block_number) {
                rank -= 1
            };

            let block_number = chunk.select(rank);

            // If our block is before the first entry in the index chunk and this first entry
            // doesn't equal to our block, it might be before the first write ever. To check, we
            // look at the previous entry and check if the key is the same.
            // This check is worth it, the `cursor.prev()` check is rarely triggered (the if will
            // short-circuit) and when it passes we save a full seek into the changeset/plain state
            // table.
            if rank == 0 &&
                block_number != Some(self.block_number) &&
                !cursor.prev()?.is_some_and(|(key, _)| key_filter(&key))
            {
                if let (Some(_), Some(block_number)) = (lowest_available_block_number, block_number)
                {
                    // The key may have been written, but due to pruning we may not have changesets
                    // and history, so we need to make a changeset lookup.
                    Ok(HistoryInfo::InChangeset(block_number))
                } else {
                    // The key is written to, but only after our block.
                    Ok(HistoryInfo::NotYetWritten)
                }
            } else if let Some(block_number) = block_number {
                // The chunk contains an entry for a write after our block, return it.
                Ok(HistoryInfo::InChangeset(block_number))
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

impl<'b, TX: DbTx> AccountReader for HistoricalStateProviderRef<'b, TX> {
    /// Get basic account information.
    fn basic_account(&self, address: Address) -> ProviderResult<Option<Account>> {
        match self.account_history_lookup(address)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(self
                .tx
                .cursor_dup_read::<tables::AccountChangeSets>()?
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

impl<'b, TX: DbTx> BlockHashReader for HistoricalStateProviderRef<'b, TX> {
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.static_file_provider.get_with_static_file_or_database(
            StaticFileSegment::Headers,
            number,
            |static_file| static_file.block_hash(number),
            || Ok(self.tx.get::<tables::CanonicalHeaders>(number)?),
        )
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.static_file_provider.get_range_with_static_file_or_database(
            StaticFileSegment::Headers,
            start..end,
            |static_file, range, _| static_file.canonical_hashes_range(range.start, range.end),
            |range, _| {
                self.tx
                    .cursor_read::<tables::CanonicalHeaders>()
                    .map(|mut cursor| {
                        cursor
                            .walk_range(range)?
                            .map(|result| result.map(|(_, hash)| hash).map_err(Into::into))
                            .collect::<ProviderResult<Vec<_>>>()
                    })?
                    .map_err(Into::into)
            },
            |_| true,
        )
    }
}

impl<'b, TX: DbTx> StateRootProvider for HistoricalStateProviderRef<'b, TX> {
    fn state_root(&self, state: &BundleState) -> ProviderResult<B256> {
        let mut revert_state = self.revert_state()?;
        revert_state.extend(HashedPostState::from_bundle_state(&state.state));
        revert_state.state_root(self.tx).map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(&self, state: &BundleState) -> ProviderResult<(B256, TrieUpdates)> {
        let mut revert_state = self.revert_state()?;
        revert_state.extend(HashedPostState::from_bundle_state(&state.state));
        revert_state
            .state_root_with_updates(self.tx)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<'b, TX: DbTx> StateProvider for HistoricalStateProviderRef<'b, TX> {
    /// Get storage.
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.storage_history_lookup(address, storage_key)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(Some(
                self.tx
                    .cursor_dup_read::<tables::StorageChangeSets>()?
                    .seek_by_key_subkey((changeset_block_number, address).into(), storage_key)?
                    .filter(|entry| entry.key == storage_key)
                    .ok_or_else(|| ProviderError::StorageChangesetNotFound {
                        block_number: changeset_block_number,
                        address,
                        storage_key: Box::new(storage_key),
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
    fn bytecode_by_hash(&self, code_hash: B256) -> ProviderResult<Option<Bytecode>> {
        self.tx.get::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }

    /// Get account and storage proofs.
    fn proof(&self, _address: Address, _keys: &[B256]) -> ProviderResult<AccountProof> {
        Err(ProviderError::StateRootNotAvailableForHistoricalBlock)
    }
}

/// State provider for a given block number.
/// For more detailed description, see [HistoricalStateProviderRef].
#[derive(Debug)]
pub struct HistoricalStateProvider<TX: DbTx> {
    /// Database transaction
    tx: TX,
    /// State at the block number is the main indexer of the state.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
    /// Static File provider
    static_file_provider: StaticFileProvider,
}

impl<TX: DbTx> HistoricalStateProvider<TX> {
    /// Create new StateProvider for historical block number
    pub fn new(
        tx: TX,
        block_number: BlockNumber,
        static_file_provider: StaticFileProvider,
    ) -> Self {
        Self { tx, block_number, lowest_available_blocks: Default::default(), static_file_provider }
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
    fn as_ref(&self) -> HistoricalStateProviderRef<'_, TX> {
        HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &self.tx,
            self.block_number,
            self.lowest_available_blocks,
            self.static_file_provider.clone(),
        )
    }
}

// Delegates all provider impls to [HistoricalStateProviderRef]
delegate_provider_impls!(HistoricalStateProvider<TX> where [TX: DbTx]);

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
        test_utils::create_test_provider_factory,
        AccountReader, HistoricalStateProvider, HistoricalStateProviderRef, StateProvider,
    };
    use reth_db::{
        models::{storage_sharded_key::StorageShardedKey, AccountBeforeTx, ShardedKey},
        tables,
        transaction::{DbTx, DbTxMut},
        BlockNumberList,
    };
    use reth_interfaces::provider::ProviderError;
    use reth_primitives::{address, b256, Account, Address, StorageEntry, B256, U256};

    const ADDRESS: Address = address!("0000000000000000000000000000000000000001");
    const HIGHER_ADDRESS: Address = address!("0000000000000000000000000000000000000005");
    const STORAGE: B256 = b256!("0000000000000000000000000000000000000000000000000000000000000001");

    fn assert_state_provider<T: StateProvider>() {}
    #[allow(dead_code)]
    fn assert_historical_state_provider<T: DbTx>() {
        assert_state_provider::<HistoricalStateProvider<T>>();
    }

    #[test]
    fn history_provider_get_account() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap().into_tx();
        let static_file_provider = factory.static_file_provider();

        tx.put::<tables::AccountsHistory>(
            ShardedKey { key: ADDRESS, highest_block_number: 7 },
            BlockNumberList::new([1, 3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::AccountsHistory>(
            ShardedKey { key: ADDRESS, highest_block_number: u64::MAX },
            BlockNumberList::new([10, 15]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::AccountsHistory>(
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
        tx.put::<tables::AccountChangeSets>(1, AccountBeforeTx { address: ADDRESS, info: None })
            .unwrap();
        tx.put::<tables::AccountChangeSets>(
            3,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at3) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(
            4,
            AccountBeforeTx { address: HIGHER_ADDRESS, info: None },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(
            7,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at7) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(
            10,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at10) },
        )
        .unwrap();
        tx.put::<tables::AccountChangeSets>(
            15,
            AccountBeforeTx { address: ADDRESS, info: Some(acc_at15) },
        )
        .unwrap();

        // setup plain state
        tx.put::<tables::PlainAccountState>(ADDRESS, acc_plain).unwrap();
        tx.put::<tables::PlainAccountState>(HIGHER_ADDRESS, higher_acc_plain).unwrap();
        tx.commit().unwrap();

        let tx = factory.provider().unwrap().into_tx();

        // run
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 2, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at3))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 3, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at3))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 4, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at7))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 7, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at7))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 9, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at10))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 10, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at10))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 11, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_at15))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 16, static_file_provider.clone())
                .basic_account(ADDRESS),
            Ok(Some(acc_plain))
        );

        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1, static_file_provider.clone())
                .basic_account(HIGHER_ADDRESS),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1000, static_file_provider)
                .basic_account(HIGHER_ADDRESS),
            Ok(Some(higher_acc_plain))
        );
    }

    #[test]
    fn history_provider_get_storage() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap().into_tx();
        let static_file_provider = factory.static_file_provider();

        tx.put::<tables::StoragesHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_block_number: 7 },
            },
            BlockNumberList::new([3, 7]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::StoragesHistory>(
            StorageShardedKey {
                address: ADDRESS,
                sharded_key: ShardedKey { key: STORAGE, highest_block_number: u64::MAX },
            },
            BlockNumberList::new([10, 15]).unwrap(),
        )
        .unwrap();
        tx.put::<tables::StoragesHistory>(
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
        tx.put::<tables::StorageChangeSets>((3, ADDRESS).into(), entry_at3).unwrap();
        tx.put::<tables::StorageChangeSets>((4, HIGHER_ADDRESS).into(), higher_entry_at4).unwrap();
        tx.put::<tables::StorageChangeSets>((7, ADDRESS).into(), entry_at7).unwrap();
        tx.put::<tables::StorageChangeSets>((10, ADDRESS).into(), entry_at10).unwrap();
        tx.put::<tables::StorageChangeSets>((15, ADDRESS).into(), entry_at15).unwrap();

        // setup plain state
        tx.put::<tables::PlainStorageState>(ADDRESS, entry_plain).unwrap();
        tx.put::<tables::PlainStorageState>(HIGHER_ADDRESS, higher_entry_plain).unwrap();
        tx.commit().unwrap();

        let tx = factory.provider().unwrap().into_tx();

        // run
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 0, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 3, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(U256::ZERO))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 4, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_at7.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 7, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_at7.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 9, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_at10.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 10, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_at10.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 11, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_at15.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 16, static_file_provider.clone())
                .storage(ADDRESS, STORAGE),
            Ok(Some(entry_plain.value))
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1, static_file_provider.clone())
                .storage(HIGHER_ADDRESS, STORAGE),
            Ok(None)
        );
        assert_eq!(
            HistoricalStateProviderRef::new(&tx, 1000, static_file_provider)
                .storage(HIGHER_ADDRESS, STORAGE),
            Ok(Some(higher_entry_plain.value))
        );
    }

    #[test]
    fn history_provider_unavailable() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap().into_tx();
        let static_file_provider = factory.static_file_provider();

        // provider block_number < lowest available block number,
        // i.e. state at provider block is pruned
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &tx,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(3),
                storage_history_block_number: Some(3),
            },
            static_file_provider.clone(),
        );
        assert_eq!(
            provider.account_history_lookup(ADDRESS),
            Err(ProviderError::StateAtBlockPruned(provider.block_number))
        );
        assert_eq!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Err(ProviderError::StateAtBlockPruned(provider.block_number))
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
            static_file_provider.clone(),
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
            static_file_provider,
        );
        assert_eq!(provider.account_history_lookup(ADDRESS), Ok(HistoryInfo::MaybeInPlainState));
        assert_eq!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Ok(HistoryInfo::MaybeInPlainState)
        );
    }
}
