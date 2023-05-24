use crate::{
    insert_canonical_block,
    post_state::{PostState, StorageChangeset},
};
use itertools::{izip, Itertools};
use reth_db::{
    common::KeyValue,
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
    database::{Database, DatabaseGAT},
    models::{
        sharded_key,
        storage_sharded_key::{self, StorageShardedKey},
        AccountBeforeTx, BlockNumberAddress, ShardedKey, StoredBlockBodyIndices,
    },
    table::Table,
    tables,
    transaction::{DbTx, DbTxMut, DbTxMutGAT},
    BlockNumberList,
};
use reth_interfaces::{db::DatabaseError as DbError, provider::ProviderError};
use reth_primitives::{
    keccak256, Account, Address, BlockHash, BlockNumber, ChainSpec, Hardfork, Header, SealedBlock,
    SealedBlockWithSenders, StageCheckpoint, StorageEntry, TransactionSigned,
    TransactionSignedEcRecovered, H256, U256,
};
use reth_trie::{StateRoot, StateRootError};
use std::{
    collections::{btree_map::Entry, BTreeMap, BTreeSet},
    fmt::Debug,
    ops::{Deref, DerefMut, Range, RangeBounds, RangeInclusive},
};

/// A container for any DB transaction that will open a new inner transaction when the current
/// one is committed.
// NOTE: This container is needed since `Transaction::commit` takes `mut self`, so methods in
// the pipeline that just take a reference will not be able to commit their transaction and let
// the pipeline continue. Is there a better way to do this?
//
// TODO: Re-evaluate if this is actually needed, this was introduced as a way to manage the
// lifetime of the `TXMut` and having a nice API for re-opening a new transaction after `commit`
pub struct Transaction<'this, DB: Database> {
    /// A handle to the DB.
    pub(crate) db: &'this DB,
    tx: Option<<DB as DatabaseGAT<'this>>::TXMut>,
}

impl<'a, DB: Database> Debug for Transaction<'a, DB> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Transaction").finish()
    }
}

impl<'a, DB: Database> Deref for Transaction<'a, DB> {
    type Target = <DB as DatabaseGAT<'a>>::TXMut;

    /// Dereference as the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref(&self) -> &Self::Target {
        self.tx.as_ref().expect("Tried getting a reference to a non-existent transaction")
    }
}

impl<'a, DB: Database> DerefMut for Transaction<'a, DB> {
    /// Dereference as a mutable reference to the inner transaction.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.tx.as_mut().expect("Tried getting a mutable reference to a non-existent transaction")
    }
}

// === Core impl ===

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
    /// Create a new container with the given database handle.
    ///
    /// A new inner transaction will be opened.
    pub fn new(db: &'this DB) -> Result<Self, DbError> {
        Ok(Self { db, tx: Some(db.tx_mut()?) })
    }

    /// Creates a new container with given database and transaction handles.
    pub fn new_raw(db: &'this DB, tx: <DB as DatabaseGAT<'this>>::TXMut) -> Self {
        Self { db, tx: Some(tx) }
    }

    /// Accessor to the internal Database
    pub fn inner(&self) -> &'this DB {
        self.db
    }

    /// Drops the current inner transaction and open a new one.
    pub fn drop(&mut self) -> Result<(), DbError> {
        if let Some(tx) = self.tx.take() {
            drop(tx);
        }

        self.tx = Some(self.db.tx_mut()?);

        Ok(())
    }

    /// Open a new inner transaction.
    pub fn open(&mut self) -> Result<(), DbError> {
        self.tx = Some(self.db.tx_mut()?);
        Ok(())
    }

    /// Close the current inner transaction.
    pub fn close(&mut self) {
        self.tx.take();
    }

    /// Commit the current inner transaction and open a new one.
    ///
    /// # Panics
    ///
    /// Panics if an inner transaction does not exist. This should never be the case unless
    /// [Transaction::close] was called without following up with a call to [Transaction::open].
    pub fn commit(&mut self) -> Result<bool, DbError> {
        let success = if let Some(tx) = self.tx.take() { tx.commit()? } else { false };
        self.tx = Some(self.db.tx_mut()?);
        Ok(success)
    }
}

// === Misc helpers ===

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
    /// Get lastest block number.
    pub fn tip_number(&self) -> Result<u64, DbError> {
        Ok(self.cursor_read::<tables::CanonicalHeaders>()?.last()?.unwrap_or_default().0)
    }

    /// Query [tables::CanonicalHeaders] table for block hash by block number
    pub fn get_block_hash(&self, block_number: BlockNumber) -> Result<BlockHash, TransactionError> {
        let hash = self
            .get::<tables::CanonicalHeaders>(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;
        Ok(hash)
    }

    /// Query the block body by number.
    pub fn block_body_indices(
        &self,
        number: BlockNumber,
    ) -> Result<StoredBlockBodyIndices, TransactionError> {
        let body = self
            .get::<tables::BlockBodyIndices>(number)?
            .ok_or(ProviderError::BlockBodyIndicesNotFound(number))?;
        Ok(body)
    }

    /// Query the block header by number
    pub fn get_header(&self, number: BlockNumber) -> Result<Header, TransactionError> {
        let header = self
            .get::<tables::Headers>(number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(number.into()))?;
        Ok(header)
    }

    /// Get the total difficulty for a block.
    pub fn get_td(&self, block: BlockNumber) -> Result<U256, TransactionError> {
        let td = self
            .get::<tables::HeaderTD>(block)?
            .ok_or(ProviderError::TotalDifficultyNotFound { number: block })?;
        Ok(td.into())
    }

    /// Unwind table by some number key.
    /// Returns number of rows unwound.
    #[inline]
    pub fn unwind_table_by_num<T>(&self, num: u64) -> Result<usize, DbError>
    where
        DB: Database,
        T: Table<Key = u64>,
    {
        self.unwind_table::<T, _>(num, |key| key)
    }

    /// Unwind the table to a provided block.
    /// Returns number of rows unwound.
    ///
    /// Note: Key is not inclusive and specified key would stay in db.
    pub(crate) fn unwind_table<T, F>(&self, key: u64, mut selector: F) -> Result<usize, DbError>
    where
        DB: Database,
        T: Table,
        F: FnMut(T::Key) -> u64,
    {
        let mut cursor = self.cursor_write::<T>()?;
        let mut reverse_walker = cursor.walk_back(None)?;
        let mut deleted = 0;

        while let Some(Ok((entry_key, _))) = reverse_walker.next() {
            if selector(entry_key.clone()) <= key {
                break
            }
            self.delete::<T>(entry_key, None)?;
            deleted += 1;
        }

        Ok(deleted)
    }

    /// Unwind a table forward by a [Walker][reth_db::abstraction::cursor::Walker] on another table
    pub fn unwind_table_by_walker<T1, T2>(&self, start_at: T1::Key) -> Result<(), DbError>
    where
        DB: Database,
        T1: Table,
        T2: Table<Key = T1::Value>,
    {
        let mut cursor = self.cursor_write::<T1>()?;
        let mut walker = cursor.walk(Some(start_at))?;
        while let Some((_, value)) = walker.next().transpose()? {
            self.delete::<T2>(value, None)?;
        }
        Ok(())
    }

    /// Load last shard and check if it is full and remove if it is not. If list is empty, last
    /// shard was full or there is no shards at all.
    fn take_last_account_shard(&self, address: Address) -> Result<Vec<u64>, TransactionError> {
        let mut cursor = self.cursor_read::<tables::AccountHistory>()?;
        let last = cursor.seek_exact(ShardedKey::new(address, u64::MAX))?;
        if let Some((shard_key, list)) = last {
            // delete old shard so new one can be inserted.
            self.delete::<tables::AccountHistory>(shard_key, None)?;
            let list = list.iter(0).map(|i| i as u64).collect::<Vec<_>>();
            return Ok(list)
        }
        Ok(Vec::new())
    }

    /// Load last shard and check if it is full and remove if it is not. If list is empty, last
    /// shard was full or there is no shards at all.
    pub fn take_last_storage_shard(
        &self,
        address: Address,
        storage_key: H256,
    ) -> Result<Vec<u64>, TransactionError> {
        let mut cursor = self.cursor_read::<tables::StorageHistory>()?;
        let last = cursor.seek_exact(StorageShardedKey::new(address, storage_key, u64::MAX))?;
        if let Some((storage_shard_key, list)) = last {
            // delete old shard so new one can be inserted.
            self.delete::<tables::StorageHistory>(storage_shard_key, None)?;
            let list = list.iter(0).map(|i| i as u64).collect::<Vec<_>>();
            return Ok(list)
        }
        Ok(Vec::new())
    }
}

// === Stages impl ===

impl<'this, DB> Transaction<'this, DB>
where
    DB: Database,
{
    /// Get range of blocks and its execution result
    pub fn get_block_and_execution_range(
        &self,
        chain_spec: &ChainSpec,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<(SealedBlockWithSenders, PostState)>, TransactionError> {
        self.get_take_block_and_execution_range::<false>(chain_spec, range)
    }

    /// Take range of blocks and its execution result
    pub fn take_block_and_execution_range(
        &self,
        chain_spec: &ChainSpec,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<(SealedBlockWithSenders, PostState)>, TransactionError> {
        self.get_take_block_and_execution_range::<true>(chain_spec, range)
    }

    /// Unwind and clear account hashing
    pub fn unwind_account_hashing(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(), TransactionError> {
        let mut hashed_accounts = self.cursor_write::<tables::HashedAccount>()?;

        // Aggregate all transition changesets and and make list of account that have been changed.
        self.cursor_read::<tables::AccountChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .rev()
            // fold all account to get the old balance/nonces and account that needs to be removed
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, Option<Account>>, (_, account_before)| {
                    accounts.insert(account_before.address, account_before.info);
                    accounts
                },
            )
            .into_iter()
            // hash addresses and collect it inside sorted BTreeMap.
            // We are doing keccak only once per address.
            .map(|(address, account)| (keccak256(address), account))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            // Apply values to HashedState (if Account is None remove it);
            .try_for_each(|(hashed_address, account)| -> Result<(), TransactionError> {
                if let Some(account) = account {
                    hashed_accounts.upsert(hashed_address, account)?;
                } else if hashed_accounts.seek_exact(hashed_address)?.is_some() {
                    hashed_accounts.delete_current()?;
                }
                Ok(())
            })?;
        Ok(())
    }

    /// Unwind and clear storage hashing
    pub fn unwind_storage_hashing(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> Result<(), TransactionError> {
        let mut hashed_storage = self.cursor_dup_write::<tables::HashedStorage>()?;

        // Aggregate all transition changesets and make list of accounts that have been changed.
        self.cursor_read::<tables::StorageChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            .rev()
            // fold all account to get the old balance/nonces and account that needs to be removed
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<(Address, H256), U256>,
                 (BlockNumberAddress((_, address)), storage_entry)| {
                    accounts.insert((address, storage_entry.key), storage_entry.value);
                    accounts
                },
            )
            .into_iter()
            // hash addresses and collect it inside sorted BTreeMap.
            // We are doing keccak only once per address.
            .map(|((address, key), value)| ((keccak256(address), keccak256(key)), value))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            // Apply values to HashedStorage (if Value is zero just remove it);
            .try_for_each(|((hashed_address, key), value)| -> Result<(), TransactionError> {
                if hashed_storage
                    .seek_by_key_subkey(hashed_address, key)?
                    .filter(|entry| entry.key == key)
                    .is_some()
                {
                    hashed_storage.delete_current()?;
                }

                if value != U256::ZERO {
                    hashed_storage.upsert(hashed_address, StorageEntry { key, value })?;
                }
                Ok(())
            })?;
        Ok(())
    }

    /// Unwind and clear account history indices
    pub fn unwind_account_history_indices(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<(), TransactionError> {
        let mut cursor = self.cursor_write::<tables::AccountHistory>()?;

        let account_changeset = self
            .cursor_read::<tables::AccountChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;

        let last_indices = account_changeset
            .into_iter()
            // reverse so we can get lowest transition id where we need to unwind account.
            .rev()
            // fold all account and get last transition index
            .fold(BTreeMap::new(), |mut accounts: BTreeMap<Address, u64>, (index, account)| {
                // we just need address and lowest transition id.
                accounts.insert(account.address, index);
                accounts
            });
        // try to unwind the index
        for (address, rem_index) in last_indices {
            let shard_part = unwind_account_history_shards::<DB>(&mut cursor, address, rem_index)?;

            // check last shard_part, if present, items needs to be reinserted.
            if !shard_part.is_empty() {
                // there are items in list
                self.put::<tables::AccountHistory>(
                    ShardedKey::new(address, u64::MAX),
                    BlockNumberList::new(shard_part)
                        .expect("There is at least one element in list and it is sorted."),
                )?;
            }
        }
        Ok(())
    }

    /// Unwind and clear storage history indices
    pub fn unwind_storage_history_indices(
        &self,
        range: Range<BlockNumberAddress>,
    ) -> Result<(), TransactionError> {
        let mut cursor = self.cursor_write::<tables::StorageHistory>()?;

        let storage_changesets = self
            .cursor_read::<tables::StorageChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;
        let last_indices = storage_changesets
            .into_iter()
            // reverse so we can get lowest transition id where we need to unwind account.
            .rev()
            // fold all storages and get last transition index
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<(Address, H256), u64>, (index, storage)| {
                    // we just need address and lowest transition id.
                    accounts.insert((index.address(), storage.key), index.block_number());
                    accounts
                },
            );
        for ((address, storage_key), rem_index) in last_indices {
            let shard_part =
                unwind_storage_history_shards::<DB>(&mut cursor, address, storage_key, rem_index)?;

            // check last shard_part, if present, items needs to be reinserted.
            if !shard_part.is_empty() {
                // there are items in list
                self.put::<tables::StorageHistory>(
                    StorageShardedKey::new(address, storage_key, u64::MAX),
                    BlockNumberList::new(shard_part)
                        .expect("There is at least one element in list and it is sorted."),
                )?;
            }
        }
        Ok(())
    }

    /// Append blocks and insert its post state.
    /// This will insert block data to all related tables and will update pipeline progress.
    pub fn append_blocks_with_post_state(
        &mut self,
        blocks: Vec<SealedBlockWithSenders>,
        state: PostState,
    ) -> Result<(), TransactionError> {
        if blocks.is_empty() {
            return Ok(())
        }
        let new_tip = blocks.last().unwrap();
        let new_tip_number = new_tip.number;

        let first_number = blocks.first().unwrap().number;

        let last = blocks.last().unwrap();
        let last_block_number = last.number;
        let last_block_hash = last.hash();
        let expected_state_root = last.state_root;

        // Insert the blocks
        for block in blocks {
            self.insert_block(block)?;
        }

        // Write state and changesets to the database.
        // Must be written after blocks because of the receipt lookup.
        state.write_to_db(self.deref_mut())?;

        self.insert_hashes(first_number..=last_block_number, last_block_hash, expected_state_root)?;

        // Update pipeline progress
        self.update_pipeline_stages(new_tip_number)?;

        Ok(())
    }

    /// Insert full block and make it canonical.
    ///
    /// This inserts the block and builds history related indexes. Once all blocks in a chain have
    /// been committed, the state root needs to be inserted separately with
    /// [`Transaction::insert_hashes`].
    ///
    /// # Note
    ///
    /// This assumes that we are using beacon consensus and that the block is post-merge, which
    /// means that the block will have no block reward.
    /// TODO do multi block insertion.
    pub fn insert_block(&mut self, block: SealedBlockWithSenders) -> Result<(), TransactionError> {
        // Header, Body, SenderRecovery, TD, TxLookup stages
        let (block, senders) = block.into_components();
        let range = block.number..=block.number;

        insert_canonical_block(self.deref_mut(), block, Some(senders)).unwrap();

        // account history stage
        {
            let indices = self.get_account_transition_ids_from_changeset(range.clone())?;
            self.insert_account_history_index(indices)?;
        }

        // storage history stage
        {
            let indices = self.get_storage_transition_ids_from_changeset(range)?;
            self.insert_storage_history_index(indices)?;
        }

        Ok(())
    }

    /// Calculate the hashes of all changed accounts and storages, and finally calculate the state
    /// root.
    ///
    /// The chain goes from `fork_block_number + 1` to `current_block_number`, and hashes are
    /// calculated from `from_transition_id` to `to_transition_id`.
    ///
    /// The resulting state root is compared with `expected_state_root`.
    pub fn insert_hashes(
        &mut self,
        range: RangeInclusive<BlockNumber>,
        end_block_hash: H256,
        expected_state_root: H256,
    ) -> Result<(), TransactionError> {
        // storage hashing stage
        {
            let lists = self.get_addresses_and_keys_of_changed_storages(range.clone())?;
            let storages = self.get_plainstate_storages(lists.into_iter())?;
            self.insert_storage_for_hashing(storages.into_iter())?;
        }

        // account hashing stage
        {
            let lists = self.get_addresses_of_changed_accounts(range.clone())?;
            let accounts = self.get_plainstate_accounts(lists.into_iter())?;
            self.insert_account_for_hashing(accounts.into_iter())?;
        }

        // merkle tree
        {
            let (state_root, trie_updates) =
                StateRoot::incremental_root_with_updates(self.deref_mut(), range.clone())?;
            if state_root != expected_state_root {
                return Err(TransactionError::StateRootMismatch {
                    got: state_root,
                    expected: expected_state_root,
                    block_number: *range.end(),
                    block_hash: end_block_hash,
                })
            }
            trie_updates.flush(self.deref_mut())?;
        }
        Ok(())
    }

    /// Return list of entries from table
    ///
    /// If TAKE is true, opened cursor would be write and it would delete all values from db.
    #[inline]
    pub fn get_or_take<T: Table, const TAKE: bool>(
        &self,
        range: impl RangeBounds<T::Key>,
    ) -> Result<Vec<KeyValue<T>>, DbError> {
        if TAKE {
            let mut cursor_write = self.cursor_write::<T>()?;
            let mut walker = cursor_write.walk_range(range)?;
            let mut items = Vec::new();
            while let Some(i) = walker.next().transpose()? {
                walker.delete_current()?;
                items.push(i)
            }
            Ok(items)
        } else {
            self.cursor_read::<T>()?.walk_range(range)?.collect::<Result<Vec<_>, _>>()
        }
    }

    /// Get requested blocks transaction with signer
    fn get_take_block_transaction_range<const TAKE: bool>(
        &self,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> Result<Vec<(BlockNumber, Vec<TransactionSignedEcRecovered>)>, TransactionError> {
        // Raad range of block bodies to get all transactions id's of this range.
        let block_bodies = self.get_or_take::<tables::BlockBodyIndices, false>(range)?;

        if block_bodies.is_empty() {
            return Ok(Vec::new())
        }

        // Compute the first and last tx ID in the range
        let first_transaction = block_bodies.first().expect("If we have headers").1.first_tx_num();
        let last_transaction = block_bodies.last().expect("Not empty").1.last_tx_num();

        // If this is the case then all of the blocks in the range are empty
        if last_transaction < first_transaction {
            return Ok(block_bodies.into_iter().map(|(n, _)| (n, Vec::new())).collect())
        }

        // Get transactions and senders
        let transactions = self
            .get_or_take::<tables::Transactions, TAKE>(first_transaction..=last_transaction)?
            .into_iter()
            .map(|(id, tx)| (id, tx.into()))
            .collect::<Vec<(u64, TransactionSigned)>>();

        let senders =
            self.get_or_take::<tables::TxSenders, TAKE>(first_transaction..=last_transaction)?;

        if TAKE {
            // Remove TxHashNumber
            let mut tx_hash_cursor = self.cursor_write::<tables::TxHashNumber>()?;
            for (_, tx) in transactions.iter() {
                if tx_hash_cursor.seek_exact(tx.hash())?.is_some() {
                    tx_hash_cursor.delete_current()?;
                }
            }

            // Remove TransactionBlock index if there are transaction present
            if !transactions.is_empty() {
                let tx_id_range = transactions.first().unwrap().0..=transactions.last().unwrap().0;
                self.get_or_take::<tables::TransactionBlock, TAKE>(tx_id_range)?;
            }
        }

        // Merge transaction into blocks
        let mut block_tx = Vec::with_capacity(block_bodies.len());
        let mut senders = senders.into_iter();
        let mut transactions = transactions.into_iter();
        for (block_number, block_body) in block_bodies {
            let mut one_block_tx = Vec::with_capacity(block_body.tx_count as usize);
            for _ in block_body.tx_num_range() {
                let tx = transactions.next();
                let sender = senders.next();

                let recovered = match (tx, sender) {
                    (Some((tx_id, tx)), Some((sender_tx_id, sender))) => {
                        if tx_id != sender_tx_id {
                            Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                        } else {
                            Ok(TransactionSignedEcRecovered::from_signed_transaction(tx, sender))
                        }
                    }
                    (Some((tx_id, _)), _) | (_, Some((tx_id, _))) => {
                        Err(ProviderError::MismatchOfTransactionAndSenderId { tx_id })
                    }
                    (None, None) => Err(ProviderError::BlockBodyTransactionCount),
                }?;
                one_block_tx.push(recovered)
            }
            block_tx.push((block_number, one_block_tx));
        }

        Ok(block_tx)
    }

    /// Return range of blocks and its execution result
    fn get_take_block_range<const TAKE: bool>(
        &self,
        chain_spec: &ChainSpec,
        range: impl RangeBounds<BlockNumber> + Clone,
    ) -> Result<Vec<SealedBlockWithSenders>, TransactionError> {
        // For block we need Headers, Bodies, Uncles, withdrawals, Transactions, Signers

        let block_headers = self.get_or_take::<tables::Headers, TAKE>(range.clone())?;
        if block_headers.is_empty() {
            return Ok(Vec::new())
        }

        let block_header_hashes =
            self.get_or_take::<tables::CanonicalHeaders, TAKE>(range.clone())?;
        let block_ommers = self.get_or_take::<tables::BlockOmmers, TAKE>(range.clone())?;
        let block_withdrawals =
            self.get_or_take::<tables::BlockWithdrawals, TAKE>(range.clone())?;

        let block_tx = self.get_take_block_transaction_range::<TAKE>(range.clone())?;

        if TAKE {
            // rm HeaderTD
            self.get_or_take::<tables::HeaderTD, TAKE>(range)?;
            // rm HeaderNumbers
            let mut header_number_cursor = self.cursor_write::<tables::HeaderNumbers>()?;
            for (_, hash) in block_header_hashes.iter() {
                if header_number_cursor.seek_exact(*hash)?.is_some() {
                    header_number_cursor.delete_current()?;
                }
            }
        }

        // merge all into block
        let block_header_iter = block_headers.into_iter();
        let block_header_hashes_iter = block_header_hashes.into_iter();
        let block_tx_iter = block_tx.into_iter();

        // Ommers can be empty for some blocks
        let mut block_ommers_iter = block_ommers.into_iter();
        let mut block_withdrawals_iter = block_withdrawals.into_iter();
        let mut block_ommers = block_ommers_iter.next();
        let mut block_withdrawals = block_withdrawals_iter.next();

        let mut blocks = Vec::new();
        for ((main_block_number, header), (_, header_hash), (_, tx)) in izip!(
            block_header_iter.into_iter(),
            block_header_hashes_iter.into_iter(),
            block_tx_iter.into_iter()
        ) {
            let header = header.seal(header_hash);

            let (body, senders) = tx.into_iter().map(|tx| tx.to_components()).unzip();

            // Ommers can be missing
            let mut ommers = Vec::new();
            if let Some((block_number, _)) = block_ommers.as_ref() {
                if *block_number == main_block_number {
                    ommers = block_ommers.take().unwrap().1.ommers;
                    block_ommers = block_ommers_iter.next();
                }
            };

            // withdrawal can be missing
            let shanghai_is_active =
                chain_spec.fork(Hardfork::Shanghai).active_at_timestamp(header.timestamp);
            let mut withdrawals = Some(Vec::new());
            if shanghai_is_active {
                if let Some((block_number, _)) = block_withdrawals.as_ref() {
                    if *block_number == main_block_number {
                        withdrawals = Some(block_withdrawals.take().unwrap().1.withdrawals);
                        block_withdrawals = block_withdrawals_iter.next();
                    }
                }
            } else {
                withdrawals = None
            }

            blocks.push(SealedBlockWithSenders {
                block: SealedBlock { header, body, ommers, withdrawals },
                senders,
            })
        }

        Ok(blocks)
    }

    /// Traverse over changesets and plain state and recreate the [`PostState`]s for the given range
    /// of blocks.
    ///
    /// 1. Iterate over the [BlockBodyIndices][tables::BlockBodyIndices] table to get all
    /// the transition indices.
    /// 2. Iterate over the [StorageChangeSet][tables::StorageChangeSet] table
    /// and the [AccountChangeSet][tables::AccountChangeSet] tables in reverse order to reconstruct
    /// the changesets.
    ///     - In order to have both the old and new values in the changesets, we also access the
    ///       plain state tables.
    /// 3. While iterating over the changeset tables, if we encounter a new account or storage slot,
    /// we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the plain state
    ///     3. Save the old value to the local state
    /// 4. While iterating over the changeset tables, if we encounter an account/storage slot we
    /// have seen before we:
    ///     1. Take the old value from the changeset
    ///     2. Take the new value from the local state
    ///     3. Set the local state to the value in the changeset
    ///
    /// If `TAKE` is `true`, the local state will be written to the plain state tables.
    /// 5. Get all receipts from table
    fn get_take_block_execution_result_range<const TAKE: bool>(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<PostState>, TransactionError> {
        if range.is_empty() {
            return Ok(Vec::new())
        }

        // We are not removing block meta as it is used to get block transitions.
        let block_bodies = self.get_or_take::<tables::BlockBodyIndices, false>(range.clone())?;

        // get transaction receipts
        let from_transaction_num =
            block_bodies.first().expect("already checked if there are blocks").1.first_tx_num();
        let to_transaction_num =
            block_bodies.last().expect("already checked if there are blocks").1.last_tx_num();
        let receipts =
            self.get_or_take::<tables::Receipts, TAKE>(from_transaction_num..=to_transaction_num)?;

        let storage_range = BlockNumberAddress::range(range.clone());

        let storage_changeset =
            self.get_or_take::<tables::StorageChangeSet, TAKE>(storage_range)?;
        let account_changeset = self.get_or_take::<tables::AccountChangeSet, TAKE>(range)?;

        // iterate previous value and get plain state value to create changeset
        // Double option around Account represent if Account state is know (first option) and
        // account is removed (Second Option)
        type LocalPlainState = BTreeMap<Address, (Option<Option<Account>>, BTreeMap<H256, U256>)>;

        let mut local_plain_state: LocalPlainState = BTreeMap::new();

        // iterate in reverse and get plain state.

        // Bundle execution changeset to its particular transaction and block
        let mut block_states =
            BTreeMap::from_iter(block_bodies.iter().map(|(num, _)| (*num, PostState::default())));

        let mut plain_accounts_cursor = self.cursor_write::<tables::PlainAccountState>()?;
        let mut plain_storage_cursor = self.cursor_dup_write::<tables::PlainStorageState>()?;

        // add account changeset changes
        for (block_number, account_before) in account_changeset.into_iter().rev() {
            let AccountBeforeTx { info: old_info, address } = account_before;
            let new_info = match local_plain_state.entry(address) {
                Entry::Vacant(entry) => {
                    let new_account = plain_accounts_cursor.seek_exact(address)?.map(|kv| kv.1);
                    entry.insert((Some(old_info), BTreeMap::new()));
                    new_account
                }
                Entry::Occupied(mut entry) => {
                    let new_account = std::mem::replace(&mut entry.get_mut().0, Some(old_info));
                    new_account.expect("As we are stacking account first, account would always be Some(Some) or Some(None)")
                }
            };

            let post_state = block_states.entry(block_number).or_default();
            match (old_info, new_info) {
                (Some(old), Some(new)) => {
                    if new != old {
                        post_state.change_account(block_number, address, old, new);
                    } else {
                        unreachable!("Junk data in database: an account changeset did not represent any change");
                    }
                }
                (None, Some(account)) =>  post_state.create_account(block_number, address, account),
                (Some(old), None) =>
                    post_state.destroy_account(block_number, address, old),
                (None, None) => unreachable!("Junk data in database: an account changeset transitioned from no account to no account"),
            };
        }

        // add storage changeset changes
        let mut storage_changes: BTreeMap<BlockNumberAddress, StorageChangeset> = BTreeMap::new();
        for (block_and_address, storage_entry) in storage_changeset.into_iter().rev() {
            let BlockNumberAddress((_, address)) = block_and_address;
            let new_storage =
                match local_plain_state.entry(address).or_default().1.entry(storage_entry.key) {
                    Entry::Vacant(entry) => {
                        let new_storage = plain_storage_cursor
                            .seek_by_key_subkey(address, storage_entry.key)?
                            .filter(|storage| storage.key == storage_entry.key)
                            .unwrap_or_default();
                        entry.insert(storage_entry.value);
                        new_storage.value
                    }
                    Entry::Occupied(mut entry) => {
                        std::mem::replace(entry.get_mut(), storage_entry.value)
                    }
                };
            storage_changes.entry(block_and_address).or_default().insert(
                U256::from_be_bytes(storage_entry.key.0),
                (storage_entry.value, new_storage),
            );
        }

        for (BlockNumberAddress((block_number, address)), storage_changeset) in
            storage_changes.into_iter()
        {
            block_states.entry(block_number).or_default().change_storage(
                block_number,
                address,
                storage_changeset,
            );
        }

        if TAKE {
            // iterate over local plain state remove all account and all storages.
            for (address, (account, storage)) in local_plain_state.into_iter() {
                // revert account
                if let Some(account) = account {
                    let existing_entry = plain_accounts_cursor.seek_exact(address)?;
                    if let Some(account) = account {
                        plain_accounts_cursor.upsert(address, account)?;
                    } else if existing_entry.is_some() {
                        plain_accounts_cursor.delete_current()?;
                    }
                }

                // revert storages
                for (storage_key, storage_value) in storage.into_iter() {
                    let storage_entry = StorageEntry { key: storage_key, value: storage_value };
                    // delete previous value
                    // TODO: This does not use dupsort features
                    if plain_storage_cursor
                        .seek_by_key_subkey(address, storage_key)?
                        .filter(|s| s.key == storage_key)
                        .is_some()
                    {
                        plain_storage_cursor.delete_current()?
                    }

                    // TODO: This does not use dupsort features
                    // insert value if needed
                    if storage_value != U256::ZERO {
                        plain_storage_cursor.upsert(address, storage_entry)?;
                    }
                }
            }
        }

        // iterate over block body and create ExecutionResult
        let mut receipt_iter = receipts.into_iter();

        // loop break if we are at the end of the blocks.
        for (block_number, block_body) in block_bodies.into_iter() {
            for _ in block_body.tx_num_range() {
                if let Some((_, receipt)) = receipt_iter.next() {
                    block_states
                        .entry(block_number)
                        .or_default()
                        .add_receipt(block_number, receipt);
                }
            }
        }
        Ok(block_states.into_values().collect())
    }

    /// Return range of blocks and its execution result
    pub fn get_take_block_and_execution_range<const TAKE: bool>(
        &self,
        chain_spec: &ChainSpec,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<Vec<(SealedBlockWithSenders, PostState)>, TransactionError> {
        if TAKE {
            let storage_range = BlockNumberAddress::range(range.clone());

            self.unwind_account_hashing(range.clone())?;
            self.unwind_account_history_indices(range.clone())?;
            self.unwind_storage_hashing(storage_range.clone())?;
            self.unwind_storage_history_indices(storage_range)?;

            // merkle tree
            let (new_state_root, trie_updates) =
                StateRoot::incremental_root_with_updates(self.deref(), range.clone())?;

            let parent_number = range.start().saturating_sub(1);
            let parent_state_root = self.get_header(parent_number)?.state_root;

            // state root should be always correct as we are reverting state.
            // but for sake of double verification we will check it again.
            if new_state_root != parent_state_root {
                let parent_hash = self.get_block_hash(parent_number)?;
                return Err(TransactionError::UnwindStateRootMismatch {
                    got: new_state_root,
                    expected: parent_state_root,
                    block_number: parent_number,
                    block_hash: parent_hash,
                })
            }
            trie_updates.flush(self.deref())?;
        }
        // get blocks
        let blocks = self.get_take_block_range::<TAKE>(chain_spec, range.clone())?;
        let unwind_to = blocks.first().map(|b| b.number.saturating_sub(1));
        // get execution res
        let execution_res = self.get_take_block_execution_result_range::<TAKE>(range.clone())?;
        // combine them
        let blocks_with_exec_result: Vec<_> =
            blocks.into_iter().zip(execution_res.into_iter()).collect();

        // remove block bodies it is needed for both get block range and get block execution results
        // that is why it is deleted afterwards.
        if TAKE {
            // rm block bodies
            self.get_or_take::<tables::BlockBodyIndices, TAKE>(range)?;

            // Update pipeline progress
            if let Some(fork_number) = unwind_to {
                self.update_pipeline_stages(fork_number)?;
            }
        }

        // return them
        Ok(blocks_with_exec_result)
    }

    /// Update all pipeline sync stage progress.
    pub fn update_pipeline_stages(
        &self,
        block_number: BlockNumber,
    ) -> Result<(), TransactionError> {
        // iterate over all existing stages in the table and update its progress.
        let mut cursor = self.cursor_write::<tables::SyncStage>()?;
        while let Some((stage_name, checkpoint)) = cursor.next()? {
            // TODO(alexey): do we want to invalidate stage-specific checkpoint data?
            cursor.upsert(stage_name, StageCheckpoint { block_number, ..checkpoint })?
        }

        Ok(())
    }

    /// Iterate over account changesets and return all account address that were changed.
    pub fn get_addresses_and_keys_of_changed_storages(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<Address, BTreeSet<H256>>, TransactionError> {
        Ok(self
            .cursor_read::<tables::StorageChangeSet>()?
            .walk_range(BlockNumberAddress::range(range))?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // fold all storages and save its old state so we can remove it from HashedStorage
            // it is needed as it is dup table.
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, BTreeSet<H256>>,
                 (BlockNumberAddress((_, address)), storage_entry)| {
                    accounts.entry(address).or_default().insert(storage_entry.key);
                    accounts
                },
            ))
    }

    /// Get plainstate storages
    #[allow(clippy::type_complexity)]
    pub fn get_plainstate_storages(
        &self,
        iter: impl IntoIterator<Item = (Address, impl IntoIterator<Item = H256>)>,
    ) -> Result<Vec<(Address, Vec<(H256, U256)>)>, TransactionError> {
        let mut plain_storage = self.cursor_dup_read::<tables::PlainStorageState>()?;

        iter.into_iter()
            .map(|(address, storage)| {
                storage
                    .into_iter()
                    .map(|key| -> Result<_, TransactionError> {
                        let ret = plain_storage
                            .seek_by_key_subkey(address, key)?
                            .filter(|v| v.key == key)
                            .unwrap_or_default();
                        Ok((key, ret.value))
                    })
                    .collect::<Result<Vec<(_, _)>, _>>()
                    .map(|storage| (address, storage))
            })
            .collect::<Result<Vec<(_, _)>, _>>()
    }

    /// iterate over storages and insert them to hashing table
    pub fn insert_storage_for_hashing(
        &self,
        storages: impl IntoIterator<Item = (Address, impl IntoIterator<Item = (H256, U256)>)>,
    ) -> Result<(), TransactionError> {
        // hash values
        let hashed = storages.into_iter().fold(BTreeMap::new(), |mut map, (address, storage)| {
            let storage = storage.into_iter().fold(BTreeMap::new(), |mut map, (key, value)| {
                map.insert(keccak256(key), value);
                map
            });
            map.insert(keccak256(address), storage);
            map
        });

        let mut hashed_storage = self.cursor_dup_write::<tables::HashedStorage>()?;
        // Hash the address and key and apply them to HashedStorage (if Storage is None
        // just remove it);
        hashed.into_iter().try_for_each(|(hashed_address, storage)| {
            storage.into_iter().try_for_each(|(key, value)| -> Result<(), TransactionError> {
                if hashed_storage
                    .seek_by_key_subkey(hashed_address, key)?
                    .filter(|entry| entry.key == key)
                    .is_some()
                {
                    hashed_storage.delete_current()?;
                }

                if value != U256::ZERO {
                    hashed_storage.upsert(hashed_address, StorageEntry { key, value })?;
                }
                Ok(())
            })
        })?;
        Ok(())
    }

    /// Iterate over account changesets and return all account address that were changed.
    pub fn get_addresses_of_changed_accounts(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeSet<Address>, TransactionError> {
        Ok(self
            .cursor_read::<tables::AccountChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?
            .into_iter()
            // fold all account to one set of changed accounts
            .fold(BTreeSet::new(), |mut accounts: BTreeSet<Address>, (_, account_before)| {
                accounts.insert(account_before.address);
                accounts
            }))
    }

    /// Get plainstate account from iterator
    pub fn get_plainstate_accounts(
        &self,
        iter: impl IntoIterator<Item = Address>,
    ) -> Result<Vec<(Address, Option<Account>)>, TransactionError> {
        let mut plain_accounts = self.cursor_read::<tables::PlainAccountState>()?;
        Ok(iter
            .into_iter()
            .map(|address| plain_accounts.seek_exact(address).map(|a| (address, a.map(|(_, v)| v))))
            .collect::<Result<Vec<_>, _>>()?)
    }

    /// iterate over accounts and insert them to hashing table
    pub fn insert_account_for_hashing(
        &self,
        accounts: impl IntoIterator<Item = (Address, Option<Account>)>,
    ) -> Result<(), TransactionError> {
        let mut hashed_accounts = self.cursor_write::<tables::HashedAccount>()?;

        let hashes_accounts = accounts.into_iter().fold(
            BTreeMap::new(),
            |mut map: BTreeMap<H256, Option<Account>>, (address, account)| {
                map.insert(keccak256(address), account);
                map
            },
        );

        hashes_accounts.into_iter().try_for_each(
            |(hashed_address, account)| -> Result<(), TransactionError> {
                if let Some(account) = account {
                    hashed_accounts.upsert(hashed_address, account)?
                } else if hashed_accounts.seek_exact(hashed_address)?.is_some() {
                    hashed_accounts.delete_current()?;
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    /// Get all transaction ids where account got changed.
    ///
    /// NOTE: Get inclusive range of blocks.
    pub fn get_storage_transition_ids_from_changeset(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<(Address, H256), Vec<u64>>, TransactionError> {
        let storage_changeset = self
            .cursor_read::<tables::StorageChangeSet>()?
            .walk_range(BlockNumberAddress::range(range))?
            .collect::<Result<Vec<_>, _>>()?;

        // fold all storages to one set of changes
        let storage_changeset_lists = storage_changeset.into_iter().fold(
            BTreeMap::new(),
            |mut storages: BTreeMap<(Address, H256), Vec<u64>>, (index, storage)| {
                storages
                    .entry((index.address(), storage.key))
                    .or_default()
                    .push(index.block_number());
                storages
            },
        );

        Ok(storage_changeset_lists)
    }

    /// Get all transaction ids where account got changed.
    ///
    /// NOTE: Get inclusive range of blocks.
    pub fn get_account_transition_ids_from_changeset(
        &self,
        range: RangeInclusive<BlockNumber>,
    ) -> Result<BTreeMap<Address, Vec<u64>>, TransactionError> {
        let account_changesets = self
            .cursor_read::<tables::AccountChangeSet>()?
            .walk_range(range)?
            .collect::<Result<Vec<_>, _>>()?;

        let account_transtions = account_changesets
            .into_iter()
            // fold all account to one set of changed accounts
            .fold(
                BTreeMap::new(),
                |mut accounts: BTreeMap<Address, Vec<u64>>, (index, account)| {
                    accounts.entry(account.address).or_default().push(index);
                    accounts
                },
            );

        Ok(account_transtions)
    }

    /// Insert storage change index to database. Used inside StorageHistoryIndex stage
    pub fn insert_storage_history_index(
        &self,
        storage_transitions: BTreeMap<(Address, H256), Vec<u64>>,
    ) -> Result<(), TransactionError> {
        for ((address, storage_key), mut indices) in storage_transitions {
            let mut last_shard = self.take_last_storage_shard(address, storage_key)?;
            last_shard.append(&mut indices);

            // chunk indices and insert them in shards of N size.
            let mut chunks = last_shard
                .iter()
                .chunks(storage_sharded_key::NUM_OF_INDICES_IN_SHARD)
                .into_iter()
                .map(|chunks| chunks.map(|i| *i as usize).collect::<Vec<usize>>())
                .collect::<Vec<_>>();
            let last_chunk = chunks.pop();

            // chunk indices and insert them in shards of N size.
            chunks.into_iter().try_for_each(|list| {
                self.put::<tables::StorageHistory>(
                    StorageShardedKey::new(
                        address,
                        storage_key,
                        *list.last().expect("Chuck does not return empty list") as BlockNumber,
                    ),
                    BlockNumberList::new(list).expect("Indices are presorted and not empty"),
                )
            })?;
            // Insert last list with u64::MAX
            if let Some(last_list) = last_chunk {
                self.put::<tables::StorageHistory>(
                    StorageShardedKey::new(address, storage_key, u64::MAX),
                    BlockNumberList::new(last_list).expect("Indices are presorted and not empty"),
                )?;
            }
        }
        Ok(())
    }

    /// Insert account change index to database. Used inside AccountHistoryIndex stage
    pub fn insert_account_history_index(
        &self,
        account_transitions: BTreeMap<Address, Vec<u64>>,
    ) -> Result<(), TransactionError> {
        // insert indexes to AccountHistory.
        for (address, mut indices) in account_transitions {
            let mut last_shard = self.take_last_account_shard(address)?;
            last_shard.append(&mut indices);
            // chunk indices and insert them in shards of N size.
            let mut chunks = last_shard
                .iter()
                .chunks(sharded_key::NUM_OF_INDICES_IN_SHARD)
                .into_iter()
                .map(|chunks| chunks.map(|i| *i as usize).collect::<Vec<usize>>())
                .collect::<Vec<_>>();
            let last_chunk = chunks.pop();

            chunks.into_iter().try_for_each(|list| {
                self.put::<tables::AccountHistory>(
                    ShardedKey::new(
                        address,
                        *list.last().expect("Chuck does not return empty list") as BlockNumber,
                    ),
                    BlockNumberList::new(list).expect("Indices are presorted and not empty"),
                )
            })?;
            // Insert last list with u64::MAX
            if let Some(last_list) = last_chunk {
                self.put::<tables::AccountHistory>(
                    ShardedKey::new(address, u64::MAX),
                    BlockNumberList::new(last_list).expect("Indices are presorted and not empty"),
                )?
            }
        }
        Ok(())
    }

    /// Return full table as Vec
    pub fn table<T: Table>(&self) -> Result<Vec<KeyValue<T>>, DbError>
    where
        T::Key: Default + Ord,
    {
        self.cursor_read::<T>()?.walk(Some(T::Key::default()))?.collect::<Result<Vec<_>, DbError>>()
    }
}

/// Unwind all history shards. For boundary shard, remove it from database and
/// return last part of shard with still valid items. If all full shard were removed, return list
/// would be empty.
fn unwind_account_history_shards<DB: Database>(
    cursor: &mut <<DB as DatabaseGAT<'_>>::TXMut as DbTxMutGAT<'_>>::CursorMut<
        tables::AccountHistory,
    >,
    address: Address,
    block_number: BlockNumber,
) -> Result<Vec<usize>, TransactionError> {
    let mut item = cursor.seek_exact(ShardedKey::new(address, u64::MAX))?;

    while let Some((sharded_key, list)) = item {
        // there is no more shard for address
        if sharded_key.key != address {
            break
        }
        cursor.delete_current()?;
        // check first item and if it is more and eq than `transition_id` delete current
        // item.
        let first = list.iter(0).next().expect("List can't empty");
        if first >= block_number as usize {
            item = cursor.prev()?;
            continue
        } else if block_number <= sharded_key.highest_block_number {
            // if first element is in scope whole list would be removed.
            // so at least this first element is present.
            return Ok(list.iter(0).take_while(|i| *i < block_number as usize).collect::<Vec<_>>())
        } else {
            let new_list = list.iter(0).collect::<Vec<_>>();
            return Ok(new_list)
        }
    }
    Ok(Vec::new())
}

/// Unwind all history shards. For boundary shard, remove it from database and
/// return last part of shard with still valid items. If all full shard were removed, return list
/// would be empty but this does not mean that there is none shard left but that there is no
/// split shards.
fn unwind_storage_history_shards<DB: Database>(
    cursor: &mut <<DB as DatabaseGAT<'_>>::TXMut as DbTxMutGAT<'_>>::CursorMut<
        tables::StorageHistory,
    >,
    address: Address,
    storage_key: H256,
    block_number: BlockNumber,
) -> Result<Vec<usize>, TransactionError> {
    let mut item = cursor.seek_exact(StorageShardedKey::new(address, storage_key, u64::MAX))?;

    while let Some((storage_sharded_key, list)) = item {
        // there is no more shard for address
        if storage_sharded_key.address != address ||
            storage_sharded_key.sharded_key.key != storage_key
        {
            // there is no more shard for address and storage_key.
            break
        }
        cursor.delete_current()?;
        // check first item and if it is more and eq than `transition_id` delete current
        // item.
        let first = list.iter(0).next().expect("List can't empty");
        if first >= block_number as usize {
            item = cursor.prev()?;
            continue
        } else if block_number <= storage_sharded_key.sharded_key.highest_block_number {
            // if first element is in scope whole list would be removed.
            // so at least this first element is present.
            return Ok(list.iter(0).take_while(|i| *i < block_number as usize).collect::<Vec<_>>())
        } else {
            return Ok(list.iter(0).collect::<Vec<_>>())
        }
    }
    Ok(Vec::new())
}

/// An error that can occur when using the transaction container
#[derive(Debug, PartialEq, Eq, Clone, thiserror::Error)]
pub enum TransactionError {
    /// The transaction encountered a database error.
    #[error(transparent)]
    Database(#[from] DbError),
    /// The transaction encountered a database integrity error.
    #[error(transparent)]
    DatabaseIntegrity(#[from] ProviderError),
    /// The trie error.
    #[error(transparent)]
    TrieError(#[from] StateRootError),
    /// Root mismatch
    #[error("Merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    StateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
    /// Root mismatch during unwind
    #[error("Unwind merkle trie root mismatch at #{block_number} ({block_hash:?}). Got: {got:?}. Expected: {expected:?}")]
    UnwindStateRootMismatch {
        /// Expected root
        expected: H256,
        /// Calculated root
        got: H256,
        /// Target block number
        block_number: BlockNumber,
        /// Block hash
        block_hash: BlockHash,
    },
}

#[cfg(test)]
mod test {
    use crate::{
        insert_canonical_block, test_utils::blocks::*, ShareableDatabase, Transaction,
        TransactionsProvider,
    };
    use reth_db::mdbx::test_utils::create_test_rw_db;
    use reth_primitives::{ChainSpecBuilder, MAINNET};
    use std::{ops::DerefMut, sync::Arc};

    #[test]
    fn insert_block_and_hashes_get_take() {
        let db = create_test_rw_db();

        // setup
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let chain_spec = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .shanghai_activated()
            .build();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        insert_canonical_block(tx.deref_mut(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&tx, data.genesis);

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        // get one block
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        let get_block = get[0].0.clone();
        let get_state = get[0].1.clone();
        assert_eq!(get_block, block1);
        assert_eq!(get_state, exec_res1);

        // take one block
        let take = tx.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&tx, genesis.clone());

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();
        tx.append_blocks_with_post_state(vec![block2.clone()], exec_res2.clone()).unwrap();
        tx.commit().unwrap();

        // Check that transactions map onto blocks correctly.
        {
            let provider = ShareableDatabase::new(tx.db, Arc::new(MAINNET.clone()));
            assert_eq!(
                provider.transaction_block(0).unwrap(),
                Some(1),
                "Transaction 0 should be in block 1"
            );
            assert_eq!(
                provider.transaction_block(1).unwrap(),
                Some(2),
                "Transaction 1 should be in block 2"
            );
            assert_eq!(
                provider.transaction_block(2).unwrap(),
                None,
                "Transaction 0 should not exist"
            );
        }

        // get second block
        let get = tx.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get[0].0, block1);
        assert_eq!(get[1].0, block2);
        assert_eq!(get[0].1, exec_res1);
        assert_eq!(get[1].1, exec_res2);

        // take two blocks
        let get = tx.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&tx, genesis);
    }

    #[test]
    fn insert_get_take_multiblocks() {
        let db = create_test_rw_db();

        // setup
        let mut tx = Transaction::new(db.as_ref()).unwrap();
        let chain_spec = ChainSpecBuilder::default()
            .chain(MAINNET.chain)
            .genesis(MAINNET.genesis.clone())
            .shanghai_activated()
            .build();

        let data = BlockChainTestData::default();
        let genesis = data.genesis.clone();
        let (block1, exec_res1) = data.blocks[0].clone();
        let (block2, exec_res2) = data.blocks[1].clone();

        insert_canonical_block(tx.deref_mut(), data.genesis.clone(), None).unwrap();

        assert_genesis_block(&tx, data.genesis);

        tx.append_blocks_with_post_state(vec![block1.clone()], exec_res1.clone()).unwrap();

        // get one block
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(get, vec![(block1.clone(), exec_res1.clone())]);

        // take one block
        let take = tx.take_block_and_execution_range(&chain_spec, 1..=1).unwrap();
        assert_eq!(take, vec![(block1.clone(), exec_res1.clone())]);
        assert_genesis_block(&tx, genesis.clone());

        // insert two blocks
        let mut merged_state = exec_res1.clone();
        merged_state.extend(exec_res2.clone());
        tx.append_blocks_with_post_state(
            vec![block1.clone(), block2.clone()],
            merged_state.clone(),
        )
        .unwrap();

        // get second block
        let get = tx.get_block_and_execution_range(&chain_spec, 2..=2).unwrap();
        assert_eq!(get, vec![(block2.clone(), exec_res2.clone())]);

        // get two blocks
        let get = tx.get_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(
            get,
            vec![(block1.clone(), exec_res1.clone()), (block2.clone(), exec_res2.clone())]
        );

        // take two blocks
        let get = tx.take_block_and_execution_range(&chain_spec, 1..=2).unwrap();
        assert_eq!(get, vec![(block1, exec_res1), (block2, exec_res2)]);

        // assert genesis state
        assert_genesis_block(&tx, genesis);
    }
}
