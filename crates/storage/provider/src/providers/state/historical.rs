use crate::{
    providers::state::macros::delegate_provider_impls, AccountReader, BlockHashReader,
    HashedPostStateProvider, ProviderError, StateProvider, StateRootProvider,
};
use alloy_eips::merge::EPOCH_SLOTS;
use alloy_primitives::{
    map::B256HashMap, Address, BlockNumber, Bytes, StorageKey, StorageValue, B256,
};
use reth_db::{tables, BlockNumberList};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::Table,
    transaction::DbTx,
};
use reth_primitives::{Account, Bytecode};
use reth_storage_api::{
    BlockNumReader, DBProvider, StateCommitmentProvider, StateProofProvider, StorageRootProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedStorage, MultiProof, MultiProofTargets, StateRoot,
    StorageMultiProof, StorageRoot, TrieInput,
};
use reth_trie_db::{
    DatabaseHashedPostState, DatabaseHashedStorage, DatabaseProof, DatabaseStateRoot,
    DatabaseStorageProof, DatabaseStorageRoot, DatabaseTrieWitness, StateCommitment,
};
use std::fmt::Debug;

/// State provider for a given block number which takes a tx reference.
///
/// Historical state provider accesses the state at the start of the provided block number.
/// It means that all changes made in the provided block number are not included.
///
/// Historical state provider reads the following tables:
/// - [`tables::AccountsHistory`]
/// - [`tables::Bytecodes`]
/// - [`tables::StoragesHistory`]
/// - [`tables::AccountChangeSets`]
/// - [`tables::StorageChangeSets`]
#[derive(Debug)]
pub struct HistoricalStateProviderRef<'b, Provider> {
    /// Database provider
    provider: &'b Provider,
    /// Block number is main index for the history state of accounts and storages.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
}

#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    NotYetWritten,
    InChangeset(u64),
    InPlainState,
    MaybeInPlainState,
}

impl<'b, Provider: DBProvider + BlockNumReader + StateCommitmentProvider>
    HistoricalStateProviderRef<'b, Provider>
{
    /// Create new `StateProvider` for historical block number
    pub fn new(provider: &'b Provider, block_number: BlockNumber) -> Self {
        Self { provider, block_number, lowest_available_blocks: Default::default() }
    }

    /// Create new `StateProvider` for historical block number and lowest block numbers at which
    /// account & storage histories are available.
    pub const fn new_with_lowest_available_blocks(
        provider: &'b Provider,
        block_number: BlockNumber,
        lowest_available_blocks: LowestAvailableBlocks,
    ) -> Self {
        Self { provider, block_number, lowest_available_blocks }
    }

    /// Lookup an account in the `AccountsHistory` table
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

    /// Lookup a storage key in the `StoragesHistory` table
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

    /// Checks and returns `true` if distance to historical block exceeds the provided limit.
    fn check_distance_against_limit(&self, limit: u64) -> ProviderResult<bool> {
        let tip = self.provider.last_block_number()?;

        Ok(tip.saturating_sub(self.block_number) > limit)
    }

    /// Retrieve revert hashed state for this history provider.
    fn revert_state(&self) -> ProviderResult<HashedPostState> {
        if !self.lowest_available_blocks.is_account_history_available(self.block_number) ||
            !self.lowest_available_blocks.is_storage_history_available(self.block_number)
        {
            return Err(ProviderError::StateAtBlockPruned(self.block_number))
        }

        if self.check_distance_against_limit(EPOCH_SLOTS)? {
            tracing::warn!(
                target: "provider::historical_sp",
                target = self.block_number,
                "Attempt to calculate state root for an old block might result in OOM"
            );
        }

        Ok(HashedPostState::from_reverts::<
            <Provider::StateCommitment as StateCommitment>::KeyHasher,
        >(self.tx(), self.block_number)?)
    }

    /// Retrieve revert hashed storage for this history provider and target address.
    fn revert_storage(&self, address: Address) -> ProviderResult<HashedStorage> {
        if !self.lowest_available_blocks.is_storage_history_available(self.block_number) {
            return Err(ProviderError::StateAtBlockPruned(self.block_number))
        }

        if self.check_distance_against_limit(EPOCH_SLOTS * 10)? {
            tracing::warn!(
                target: "provider::historical_sp",
                target = self.block_number,
                "Attempt to calculate storage root for an old block might result in OOM"
            );
        }

        Ok(HashedStorage::from_reverts(self.tx(), address, self.block_number)?)
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
        let mut cursor = self.tx().cursor_read::<T>()?;

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

    /// Set the lowest block number at which the account history is available.
    pub const fn with_lowest_available_account_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.account_history_block_number = Some(block_number);
        self
    }

    /// Set the lowest block number at which the storage history is available.
    pub const fn with_lowest_available_storage_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.storage_history_block_number = Some(block_number);
        self
    }
}

impl<Provider: DBProvider + BlockNumReader> HistoricalStateProviderRef<'_, Provider> {
    fn tx(&self) -> &Provider::Tx {
        self.provider.tx_ref()
    }
}

impl<Provider: DBProvider + BlockNumReader + StateCommitmentProvider> AccountReader
    for HistoricalStateProviderRef<'_, Provider>
{
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        match self.account_history_lookup(*address)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(self
                .tx()
                .cursor_dup_read::<tables::AccountChangeSets>()?
                .seek_by_key_subkey(changeset_block_number, *address)?
                .filter(|acc| &acc.address == address)
                .ok_or(ProviderError::AccountChangesetNotFound {
                    block_number: changeset_block_number,
                    address: *address,
                })?
                .info),
            HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => {
                Ok(self.tx().get_by_encoded_key::<tables::PlainAccountState>(address)?)
            }
        }
    }
}

impl<Provider: DBProvider + BlockNumReader + BlockHashReader> BlockHashReader
    for HistoricalStateProviderRef<'_, Provider>
{
    /// Get block hash by number.
    fn block_hash(&self, number: u64) -> ProviderResult<Option<B256>> {
        self.provider.block_hash(number)
    }

    fn canonical_hashes_range(
        &self,
        start: BlockNumber,
        end: BlockNumber,
    ) -> ProviderResult<Vec<B256>> {
        self.provider.canonical_hashes_range(start, end)
    }
}

impl<Provider: DBProvider + BlockNumReader + StateCommitmentProvider> StateRootProvider
    for HistoricalStateProviderRef<'_, Provider>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let mut revert_state = self.revert_state()?;
        revert_state.extend(hashed_state);
        StateRoot::overlay_root(self.tx(), revert_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes(&self, mut input: TrieInput) -> ProviderResult<B256> {
        input.prepend(self.revert_state()?);
        StateRoot::overlay_root_from_nodes(self.tx(), input)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let mut revert_state = self.revert_state()?;
        revert_state.extend(hashed_state);
        StateRoot::overlay_root_with_updates(self.tx(), revert_state)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn state_root_from_nodes_with_updates(
        &self,
        mut input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        input.prepend(self.revert_state()?);
        StateRoot::overlay_root_from_nodes_with_updates(self.tx(), input)
            .map_err(|err| ProviderError::Database(err.into()))
    }
}

impl<Provider: DBProvider + BlockNumReader + StateCommitmentProvider> StorageRootProvider
    for HistoricalStateProviderRef<'_, Provider>
{
    fn storage_root(
        &self,
        address: Address,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<B256> {
        let mut revert_storage = self.revert_storage(address)?;
        revert_storage.extend(&hashed_storage);
        StorageRoot::overlay_root(self.tx(), address, revert_storage)
            .map_err(|err| ProviderError::Database(err.into()))
    }

    fn storage_proof(
        &self,
        address: Address,
        slot: B256,
        hashed_storage: HashedStorage,
    ) -> ProviderResult<reth_trie::StorageProof> {
        let mut revert_storage = self.revert_storage(address)?;
        revert_storage.extend(&hashed_storage);
        StorageProof::overlay_storage_proof(self.tx(), address, slot, revert_storage)
            .map_err(ProviderError::from)
    }

    fn storage_multiproof(
        &self,
        address: Address,
        slots: &[B256],
        hashed_storage: HashedStorage,
    ) -> ProviderResult<StorageMultiProof> {
        let mut revert_storage = self.revert_storage(address)?;
        revert_storage.extend(&hashed_storage);
        StorageProof::overlay_storage_multiproof(self.tx(), address, slots, revert_storage)
            .map_err(ProviderError::from)
    }
}

impl<Provider: DBProvider + BlockNumReader + StateCommitmentProvider> StateProofProvider
    for HistoricalStateProviderRef<'_, Provider>
{
    /// Get account and storage proofs.
    fn proof(
        &self,
        mut input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        input.prepend(self.revert_state()?);
        Proof::overlay_account_proof(self.tx(), input, address, slots).map_err(ProviderError::from)
    }

    fn multiproof(
        &self,
        mut input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        input.prepend(self.revert_state()?);
        Proof::overlay_multiproof(self.tx(), input, targets).map_err(ProviderError::from)
    }

    fn witness(
        &self,
        mut input: TrieInput,
        target: HashedPostState,
    ) -> ProviderResult<B256HashMap<Bytes>> {
        input.prepend(self.revert_state()?);
        TrieWitness::overlay_witness(self.tx(), input, target).map_err(ProviderError::from)
    }
}

impl<Provider: StateCommitmentProvider> HashedPostStateProvider
    for HistoricalStateProviderRef<'_, Provider>
{
    fn hashed_post_state(&self, bundle_state: &revm::db::BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<
            <Provider::StateCommitment as StateCommitment>::KeyHasher,
        >(bundle_state.state())
    }
}

impl<Provider: DBProvider + BlockNumReader + BlockHashReader + StateCommitmentProvider>
    StateProvider for HistoricalStateProviderRef<'_, Provider>
{
    /// Get storage.
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.storage_history_lookup(address, storage_key)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => Ok(Some(
                self.tx()
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
                .tx()
                .cursor_dup_read::<tables::PlainStorageState>()?
                .seek_by_key_subkey(address, storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(|entry| entry.value)
                .or(Some(StorageValue::ZERO))),
        }
    }

    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.tx().get_by_encoded_key::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
}

impl<Provider: StateCommitmentProvider> StateCommitmentProvider
    for HistoricalStateProviderRef<'_, Provider>
{
    type StateCommitment = Provider::StateCommitment;
}

/// State provider for a given block number.
/// For more detailed description, see [`HistoricalStateProviderRef`].
#[derive(Debug)]
pub struct HistoricalStateProvider<Provider> {
    /// Database provider.
    provider: Provider,
    /// State at the block number is the main indexer of the state.
    block_number: BlockNumber,
    /// Lowest blocks at which different parts of the state are available.
    lowest_available_blocks: LowestAvailableBlocks,
}

impl<Provider: DBProvider + BlockNumReader + StateCommitmentProvider>
    HistoricalStateProvider<Provider>
{
    /// Create new `StateProvider` for historical block number
    pub fn new(provider: Provider, block_number: BlockNumber) -> Self {
        Self { provider, block_number, lowest_available_blocks: Default::default() }
    }

    /// Set the lowest block number at which the account history is available.
    pub const fn with_lowest_available_account_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.account_history_block_number = Some(block_number);
        self
    }

    /// Set the lowest block number at which the storage history is available.
    pub const fn with_lowest_available_storage_history_block_number(
        mut self,
        block_number: BlockNumber,
    ) -> Self {
        self.lowest_available_blocks.storage_history_block_number = Some(block_number);
        self
    }

    /// Returns a new provider that takes the `TX` as reference
    #[inline(always)]
    const fn as_ref(&self) -> HistoricalStateProviderRef<'_, Provider> {
        HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &self.provider,
            self.block_number,
            self.lowest_available_blocks,
        )
    }
}

impl<Provider: StateCommitmentProvider> StateCommitmentProvider
    for HistoricalStateProvider<Provider>
{
    type StateCommitment = Provider::StateCommitment;
}

// Delegates all provider impls to [HistoricalStateProviderRef]
delegate_provider_impls!(HistoricalStateProvider<Provider> where [Provider: DBProvider + BlockNumReader + BlockHashReader + StateCommitmentProvider]);

/// Lowest blocks at which different parts of the state are available.
/// They may be [Some] if pruning is enabled.
#[derive(Clone, Copy, Debug, Default)]
pub struct LowestAvailableBlocks {
    /// Lowest block number at which the account history is available. It may not be available if
    /// [`reth_prune_types::PruneSegment::AccountHistory`] was pruned.
    /// [`Option::None`] means all history is available.
    pub account_history_block_number: Option<BlockNumber>,
    /// Lowest block number at which the storage history is available. It may not be available if
    /// [`reth_prune_types::PruneSegment::StorageHistory`] was pruned.
    /// [`Option::None`] means all history is available.
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
    use alloy_primitives::{address, b256, Address, B256, U256};
    use reth_db::{tables, BlockNumberList};
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, AccountBeforeTx, ShardedKey},
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives::{Account, StorageEntry};
    use reth_storage_api::{
        BlockHashReader, BlockNumReader, DBProvider, DatabaseProviderFactory,
        StateCommitmentProvider,
    };
    use reth_storage_errors::provider::ProviderError;

    const ADDRESS: Address = address!("0000000000000000000000000000000000000001");
    const HIGHER_ADDRESS: Address = address!("0000000000000000000000000000000000000005");
    const STORAGE: B256 = b256!("0000000000000000000000000000000000000000000000000000000000000001");

    const fn assert_state_provider<T: StateProvider>() {}
    #[allow(dead_code)]
    const fn assert_historical_state_provider<
        T: DBProvider + BlockNumReader + BlockHashReader + StateCommitmentProvider,
    >() {
        assert_state_provider::<HistoricalStateProvider<T>>();
    }

    #[test]
    fn history_provider_get_account() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap().into_tx();

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

        let db = factory.provider().unwrap();

        // run
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 1).basic_account(&ADDRESS),
            Ok(None)
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 2).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at3
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 3).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at3
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 4).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at7
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 7).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at7
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 9).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at10
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 10).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at10
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 11).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_at15
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 16).basic_account(&ADDRESS),
            Ok(Some(acc)) if acc == acc_plain
        ));

        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 1).basic_account(&HIGHER_ADDRESS),
            Ok(None)
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 1000).basic_account(&HIGHER_ADDRESS),
            Ok(Some(acc)) if acc == higher_acc_plain
        ));
    }

    #[test]
    fn history_provider_get_storage() {
        let factory = create_test_provider_factory();
        let tx = factory.provider_rw().unwrap().into_tx();

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

        let db = factory.provider().unwrap();

        // run
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 0).storage(ADDRESS, STORAGE),
            Ok(None)
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 3).storage(ADDRESS, STORAGE),
            Ok(Some(U256::ZERO))
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 4).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_at7.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 7).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_at7.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 9).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_at10.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 10).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_at10.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 11).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_at15.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 16).storage(ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == entry_plain.value
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 1).storage(HIGHER_ADDRESS, STORAGE),
            Ok(None)
        ));
        assert!(matches!(
            HistoricalStateProviderRef::new(&db, 1000).storage(HIGHER_ADDRESS, STORAGE),
            Ok(Some(expected_value)) if expected_value == higher_entry_plain.value
        ));
    }

    #[test]
    fn history_provider_unavailable() {
        let factory = create_test_provider_factory();
        let db = factory.database_provider_rw().unwrap();

        // provider block_number < lowest available block number,
        // i.e. state at provider block is pruned
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &db,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(3),
                storage_history_block_number: Some(3),
            },
        );
        assert!(matches!(
            provider.account_history_lookup(ADDRESS),
            Err(ProviderError::StateAtBlockPruned(number)) if number == provider.block_number
        ));
        assert!(matches!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Err(ProviderError::StateAtBlockPruned(number)) if number == provider.block_number
        ));

        // provider block_number == lowest available block number,
        // i.e. state at provider block is available
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &db,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(2),
                storage_history_block_number: Some(2),
            },
        );
        assert!(matches!(
            provider.account_history_lookup(ADDRESS),
            Ok(HistoryInfo::MaybeInPlainState)
        ));
        assert!(matches!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Ok(HistoryInfo::MaybeInPlainState)
        ));

        // provider block_number == lowest available block number,
        // i.e. state at provider block is available
        let provider = HistoricalStateProviderRef::new_with_lowest_available_blocks(
            &db,
            2,
            LowestAvailableBlocks {
                account_history_block_number: Some(1),
                storage_history_block_number: Some(1),
            },
        );
        assert!(matches!(
            provider.account_history_lookup(ADDRESS),
            Ok(HistoryInfo::MaybeInPlainState)
        ));
        assert!(matches!(
            provider.storage_history_lookup(ADDRESS, STORAGE),
            Ok(HistoryInfo::MaybeInPlainState)
        ));
    }
}
