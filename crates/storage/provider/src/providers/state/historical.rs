use crate::{
    AccountReader, BlockHashReader, ChangeSetReader, HashedPostStateProvider, ProviderError,
    StateProvider, StateRootProvider,
};
use alloy_eips::merge::EPOCH_SLOTS;
use alloy_primitives::{Address, BlockNumber, Bytes, StorageKey, StorageValue, B256};
use reth_db_api::{
    cursor::{DbCursorRO, DbDupCursorRO},
    models::{storage_sharded_key::StorageShardedKey, ShardedKey},
    table::Table,
    tables,
    transaction::DbTx,
    BlockNumberList,
};
use reth_primitives_traits::{Account, Bytecode};
use reth_storage_api::{
    BlockNumReader, BytecodeReader, DBProvider, StateProofProvider, StorageChangeSetReader,
    StorageRootProvider,
};
use reth_storage_errors::provider::ProviderResult;
use reth_trie::{
    proof::{Proof, StorageProof},
    updates::TrieUpdates,
    witness::TrieWitness,
    AccountProof, HashedPostState, HashedPostStateSorted, HashedStorage, KeccakKeyHasher,
    MultiProof, MultiProofTargets, StateRoot, StorageMultiProof, StorageRoot, TrieInput,
    TrieInputSorted,
};
use reth_trie_db::{
    hashed_storage_from_reverts_with_provider, DatabaseHashedPostState, DatabaseProof,
    DatabaseStateRoot, DatabaseStorageProof, DatabaseStorageRoot, DatabaseTrieWitness,
};

use std::fmt::Debug;

/// Result of a history lookup for an account or storage slot.
///
/// Indicates where to find the historical value for a given key at a specific block.
#[derive(Debug, Eq, PartialEq)]
pub enum HistoryInfo {
    /// The key is written to, but only after our block (not yet written at the target block). Or
    /// it has never been written.
    NotYetWritten,
    /// The chunk contains an entry for a write after our block at the given block number.
    /// The value should be looked up in the changeset at this block.
    InChangeset(u64),
    /// The chunk does not contain an entry for a write after our block. This can only
    /// happen if this is the last chunk, so we need to look in the plain state.
    InPlainState,
    /// The key may have been written, but due to pruning we may not have changesets and
    /// history, so we need to make a plain state lookup.
    MaybeInPlainState,
}

impl HistoryInfo {
    /// Determines where to find the historical value based on computed shard lookup results.
    ///
    /// This is a pure function shared by both MDBX and `RocksDB` backends.
    ///
    /// # Arguments
    /// * `found_block` - The block number from the shard lookup
    /// * `is_before_first_write` - True if the target block is before the first write to this key.
    ///   This should be computed as: `rank == 0 && found_block != Some(block_number) &&
    ///   !has_previous_shard` where `has_previous_shard` comes from a lazy `cursor.prev()` check.
    /// * `lowest_available` - Lowest block where history is available (pruning boundary)
    pub const fn from_lookup(
        found_block: Option<u64>,
        is_before_first_write: bool,
        lowest_available: Option<BlockNumber>,
    ) -> Self {
        if is_before_first_write {
            if let (Some(_), Some(block_number)) = (lowest_available, found_block) {
                // The key may have been written, but due to pruning we may not have changesets
                // and history, so we need to make a changeset lookup.
                return Self::InChangeset(block_number)
            }
            // The key is written to, but only after our block.
            return Self::NotYetWritten
        }

        if let Some(block_number) = found_block {
            // The chunk contains an entry for a write after our block, return it.
            Self::InChangeset(block_number)
        } else {
            // The chunk does not contain an entry for a write after our block. This can only
            // happen if this is the last chunk and so we need to look in the plain state.
            Self::InPlainState
        }
    }
}

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

impl<'b, Provider: DBProvider + ChangeSetReader + StorageChangeSetReader + BlockNumReader>
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
    fn revert_state(&self) -> ProviderResult<HashedPostStateSorted> {
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

        HashedPostStateSorted::from_reverts::<KeccakKeyHasher>(self.provider, self.block_number..)
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

        hashed_storage_from_reverts_with_provider(self.provider, address, self.block_number)
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

        // Lookup the history chunk in the history index. If the key does not appear in the
        // index, the first chunk for the next key will be returned so we filter out chunks that
        // have a different key.
        if let Some(chunk) = cursor.seek(key)?.filter(|(key, _)| key_filter(key)).map(|x| x.1) {
            // Get the rank of the first entry before or equal to our block.
            let mut rank = chunk.rank(self.block_number);

            // Adjust the rank, so that we have the rank of the first entry strictly before our
            // block (not equal to it).
            if rank.checked_sub(1).and_then(|r| chunk.select(r)) == Some(self.block_number) {
                rank -= 1;
            }

            let found_block = chunk.select(rank);

            // If our block is before the first entry in the index chunk and this first entry
            // doesn't equal to our block, it might be before the first write ever. To check, we
            // look at the previous entry and check if the key is the same.
            // This check is worth it, the `cursor.prev()` check is rarely triggered (the if will
            // short-circuit) and when it passes we save a full seek into the changeset/plain state
            // table.
            let is_before_first_write =
                needs_prev_shard_check(rank, found_block, self.block_number) &&
                    !cursor.prev()?.is_some_and(|(key, _)| key_filter(&key));

            Ok(HistoryInfo::from_lookup(
                found_block,
                is_before_first_write,
                lowest_available_block_number,
            ))
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

impl<Provider: DBProvider + BlockNumReader + ChangeSetReader + StorageChangeSetReader> AccountReader
    for HistoricalStateProviderRef<'_, Provider>
{
    /// Get basic account information.
    fn basic_account(&self, address: &Address) -> ProviderResult<Option<Account>> {
        match self.account_history_lookup(*address)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => {
                // Use ChangeSetReader trait method to get the account from changesets
                self.provider
                    .get_account_before_block(changeset_block_number, *address)?
                    .ok_or(ProviderError::AccountChangesetNotFound {
                        block_number: changeset_block_number,
                        address: *address,
                    })
                    .map(|account_before| account_before.info)
            }
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

impl<Provider: DBProvider + ChangeSetReader + StorageChangeSetReader + BlockNumReader>
    StateRootProvider for HistoricalStateProviderRef<'_, Provider>
{
    fn state_root(&self, hashed_state: HashedPostState) -> ProviderResult<B256> {
        let mut revert_state = self.revert_state()?;
        let hashed_state_sorted = hashed_state.into_sorted();
        revert_state.extend_ref(&hashed_state_sorted);
        Ok(StateRoot::overlay_root(self.tx(), &revert_state)?)
    }

    fn state_root_from_nodes(&self, mut input: TrieInput) -> ProviderResult<B256> {
        input.prepend(self.revert_state()?.into());
        Ok(StateRoot::overlay_root_from_nodes(self.tx(), TrieInputSorted::from_unsorted(input))?)
    }

    fn state_root_with_updates(
        &self,
        hashed_state: HashedPostState,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        let mut revert_state = self.revert_state()?;
        let hashed_state_sorted = hashed_state.into_sorted();
        revert_state.extend_ref(&hashed_state_sorted);
        Ok(StateRoot::overlay_root_with_updates(self.tx(), &revert_state)?)
    }

    fn state_root_from_nodes_with_updates(
        &self,
        mut input: TrieInput,
    ) -> ProviderResult<(B256, TrieUpdates)> {
        input.prepend(self.revert_state()?.into());
        Ok(StateRoot::overlay_root_from_nodes_with_updates(
            self.tx(),
            TrieInputSorted::from_unsorted(input),
        )?)
    }
}

impl<Provider: DBProvider + ChangeSetReader + StorageChangeSetReader + BlockNumReader>
    StorageRootProvider for HistoricalStateProviderRef<'_, Provider>
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

impl<Provider: DBProvider + ChangeSetReader + StorageChangeSetReader + BlockNumReader>
    StateProofProvider for HistoricalStateProviderRef<'_, Provider>
{
    /// Get account and storage proofs.
    fn proof(
        &self,
        mut input: TrieInput,
        address: Address,
        slots: &[B256],
    ) -> ProviderResult<AccountProof> {
        input.prepend(self.revert_state()?.into());
        let proof = <Proof<_, _> as DatabaseProof>::from_tx(self.tx());
        proof.overlay_account_proof(input, address, slots).map_err(ProviderError::from)
    }

    fn multiproof(
        &self,
        mut input: TrieInput,
        targets: MultiProofTargets,
    ) -> ProviderResult<MultiProof> {
        input.prepend(self.revert_state()?.into());
        let proof = <Proof<_, _> as DatabaseProof>::from_tx(self.tx());
        proof.overlay_multiproof(input, targets).map_err(ProviderError::from)
    }

    fn witness(&self, mut input: TrieInput, target: HashedPostState) -> ProviderResult<Vec<Bytes>> {
        input.prepend(self.revert_state()?.into());
        TrieWitness::overlay_witness(self.tx(), input, target)
            .map_err(ProviderError::from)
            .map(|hm| hm.into_values().collect())
    }
}

impl<Provider> HashedPostStateProvider for HistoricalStateProviderRef<'_, Provider> {
    fn hashed_post_state(&self, bundle_state: &revm_database::BundleState) -> HashedPostState {
        HashedPostState::from_bundle_state::<KeccakKeyHasher>(bundle_state.state())
    }
}

impl<
        Provider: DBProvider + BlockNumReader + BlockHashReader + ChangeSetReader + StorageChangeSetReader,
    > StateProvider for HistoricalStateProviderRef<'_, Provider>
{
    /// Get storage.
    fn storage(
        &self,
        address: Address,
        storage_key: StorageKey,
    ) -> ProviderResult<Option<StorageValue>> {
        match self.storage_history_lookup(address, storage_key)? {
            HistoryInfo::NotYetWritten => Ok(None),
            HistoryInfo::InChangeset(changeset_block_number) => self
                .provider
                .get_storage_before_block(changeset_block_number, address, storage_key)?
                .ok_or_else(|| ProviderError::StorageChangesetNotFound {
                    block_number: changeset_block_number,
                    address,
                    storage_key: Box::new(storage_key),
                })
                .map(|entry| entry.value)
                .map(Some),
            HistoryInfo::InPlainState | HistoryInfo::MaybeInPlainState => Ok(self
                .tx()
                .cursor_dup_read::<tables::PlainStorageState>()?
                .seek_by_key_subkey(address, storage_key)?
                .filter(|entry| entry.key == storage_key)
                .map(|entry| entry.value)
                .or(Some(StorageValue::ZERO))),
        }
    }
}

impl<Provider: DBProvider + BlockNumReader> BytecodeReader
    for HistoricalStateProviderRef<'_, Provider>
{
    /// Get account code by its hash
    fn bytecode_by_hash(&self, code_hash: &B256) -> ProviderResult<Option<Bytecode>> {
        self.tx().get_by_encoded_key::<tables::Bytecodes>(code_hash).map_err(Into::into)
    }
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

impl<Provider: DBProvider + ChangeSetReader + StorageChangeSetReader + BlockNumReader>
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

// Delegates all provider impls to [HistoricalStateProviderRef]
reth_storage_api::macros::delegate_provider_impls!(HistoricalStateProvider<Provider> where [Provider: DBProvider + BlockNumReader + BlockHashReader + ChangeSetReader + StorageChangeSetReader]);

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

/// Checks if a previous shard lookup is needed to determine if we're before the first write.
///
/// Returns `true` when `rank == 0` (first entry in shard) and the found block doesn't match
/// the target block number. In this case, we need to check if there's a previous shard.
#[inline]
pub fn needs_prev_shard_check(
    rank: u64,
    found_block: Option<u64>,
    block_number: BlockNumber,
) -> bool {
    rank == 0 && found_block != Some(block_number)
}

#[cfg(test)]
mod tests {
    use super::needs_prev_shard_check;
    use crate::{
        providers::state::historical::{HistoryInfo, LowestAvailableBlocks},
        test_utils::create_test_provider_factory,
        AccountReader, HistoricalStateProvider, HistoricalStateProviderRef, StateProvider,
    };
    use alloy_primitives::{address, b256, Address, B256, U256};
    use reth_db_api::{
        models::{storage_sharded_key::StorageShardedKey, AccountBeforeTx, ShardedKey},
        tables,
        transaction::{DbTx, DbTxMut},
        BlockNumberList,
    };
    use reth_primitives_traits::{Account, StorageEntry};
    use reth_storage_api::{
        BlockHashReader, BlockNumReader, ChangeSetReader, DBProvider, DatabaseProviderFactory,
        StorageChangeSetReader,
    };
    use reth_storage_errors::provider::ProviderError;

    const ADDRESS: Address = address!("0x0000000000000000000000000000000000000001");
    const HIGHER_ADDRESS: Address = address!("0x0000000000000000000000000000000000000005");
    const STORAGE: B256 =
        b256!("0x0000000000000000000000000000000000000000000000000000000000000001");

    const fn assert_state_provider<T: StateProvider>() {}
    #[expect(dead_code)]
    const fn assert_historical_state_provider<
        T: DBProvider + BlockNumReader + BlockHashReader + ChangeSetReader + StorageChangeSetReader,
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

    #[test]
    fn test_history_info_from_lookup() {
        // Before first write, no pruning → not yet written
        assert_eq!(HistoryInfo::from_lookup(Some(10), true, None), HistoryInfo::NotYetWritten);
        assert_eq!(HistoryInfo::from_lookup(None, true, None), HistoryInfo::NotYetWritten);

        // Before first write WITH pruning → check changeset (pruning may have removed history)
        assert_eq!(HistoryInfo::from_lookup(Some(10), true, Some(5)), HistoryInfo::InChangeset(10));
        assert_eq!(HistoryInfo::from_lookup(None, true, Some(5)), HistoryInfo::NotYetWritten);

        // Not before first write → check changeset or plain state
        assert_eq!(HistoryInfo::from_lookup(Some(10), false, None), HistoryInfo::InChangeset(10));
        assert_eq!(HistoryInfo::from_lookup(None, false, None), HistoryInfo::InPlainState);
    }

    #[test]
    fn test_needs_prev_shard_check() {
        // Only needs check when rank == 0 and found_block != block_number
        assert!(needs_prev_shard_check(0, Some(10), 5));
        assert!(needs_prev_shard_check(0, None, 5));
        assert!(!needs_prev_shard_check(0, Some(5), 5)); // found_block == block_number
        assert!(!needs_prev_shard_check(1, Some(10), 5)); // rank > 0
    }
}
