use crate::{
    providers::StaticFileProviderRWRefMut, DatabaseProvider, DatabaseProviderRO,
    DatabaseProviderRW, StateChangeWriter, StateWriter, TrieWriter,
};
use itertools::Itertools;
use reth_db::{
    cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO, DbDupCursorRW},
    tables,
    transaction::{DbTx, DbTxMut},
    Database,
};
use reth_errors::{ProviderError, ProviderResult};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{
    BlockNumber, Header, StaticFileSegment, StorageEntry, TransactionSignedNoHash, B256, U256,
};
use reth_storage_api::ReceiptWriter;
use reth_storage_errors::writer::StorageWriterError;
use reth_trie::{updates::TrieUpdates, HashedPostStateSorted};
use revm::db::OriginalValuesKnown;
use static_file::StaticFileWriter;
use std::borrow::Borrow;

mod database;
mod static_file;
use database::DatabaseWriter;

enum StorageType<C = (), S = ()> {
    Database(C),
    StaticFile(S),
}

/// [`StorageWriter`] is responsible for managing the writing to either database, static file or
/// both.
#[derive(Debug)]
pub struct StorageWriter<'a, 'b, TX> {
    database_writer: Option<&'a DatabaseProvider<TX>>,
    static_file_writer: Option<StaticFileProviderRWRefMut<'b>>,
}

impl<'a, 'b, TX> StorageWriter<'a, 'b, TX> {
    /// Creates a new instance of [`StorageWriter`].
    ///
    /// # Parameters
    /// - `database_writer`: An optional reference to a database writer.
    /// - `static_file_writer`: An optional mutable reference to a static file writer.
    pub const fn new(
        database_writer: Option<&'a DatabaseProvider<TX>>,
        static_file_writer: Option<StaticFileProviderRWRefMut<'b>>,
    ) -> Self {
        Self { database_writer, static_file_writer }
    }

    /// Creates a new instance of [`StorageWriter`] from a static file writer.
    pub const fn from_static_file_writer(
        static_file_writer: StaticFileProviderRWRefMut<'b>,
    ) -> Self {
        Self::new(None, Some(static_file_writer))
    }

    /// Creates a new instance of [`StorageWriter`] from a read-only database provider.
    pub const fn from_database_provider_ro<DB>(
        database: &'a DatabaseProviderRO<DB>,
    ) -> StorageWriter<'_, '_, <DB as Database>::TX>
    where
        DB: Database,
    {
        StorageWriter::new(Some(database), None)
    }

    /// Creates a new instance of [`StorageWriter`] from a read-write database provider.
    pub fn from_database_provider_rw<DB>(
        database: &'a DatabaseProviderRW<DB>,
    ) -> StorageWriter<'_, '_, <DB as Database>::TXMut>
    where
        DB: Database,
    {
        StorageWriter::new(Some(database), None)
    }

    /// Returns a reference to the database writer.
    ///
    /// # Panics
    /// If the database writer is not set.
    fn database_writer(&self) -> &DatabaseProvider<TX> {
        self.database_writer.as_ref().expect("should exist")
    }

    /// Returns a mutable reference to the static file writer.
    ///
    /// # Panics
    /// If the static file writer is not set.
    fn static_file_writer(&mut self) -> &mut StaticFileProviderRWRefMut<'b> {
        self.static_file_writer.as_mut().expect("should exist")
    }

    /// Ensures that the database writer is set.
    ///
    /// # Returns
    /// - `Ok(())` if the database writer is set.
    /// - `Err(StorageWriterError::MissingDatabaseWriter)` if the database writer is not set.
    const fn ensure_database_writer(&self) -> Result<(), StorageWriterError> {
        if self.database_writer.is_none() {
            return Err(StorageWriterError::MissingDatabaseWriter)
        }
        Ok(())
    }

    /// Ensures that the static file writer is set.
    ///
    /// # Returns
    /// - `Ok(())` if the static file writer is set.
    /// - `Err(StorageWriterError::MissingStaticFileWriter)` if the static file writer is not set.
    const fn ensure_static_file_writer(&self) -> Result<(), StorageWriterError> {
        if self.static_file_writer.is_none() {
            return Err(StorageWriterError::MissingStaticFileWriter)
        }
        Ok(())
    }
}

impl<'a, 'b, TX> StorageWriter<'a, 'b, TX>
where
    TX: DbTx,
{
    /// Appends headers to static files, using the
    /// [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties) table to determine the
    /// total difficulty of the parent block during header insertion.
    ///
    /// NOTE: The static file writer used to construct this [`StorageWriter`] MUST be a writer for
    /// the Headers segment.
    pub fn append_headers_from_blocks<H, I>(
        &mut self,
        initial_block_number: BlockNumber,
        headers: impl Iterator<Item = I>,
    ) -> ProviderResult<()>
    where
        I: Borrow<(H, B256)>,
        H: Borrow<Header>,
    {
        self.ensure_database_writer()?;
        self.ensure_static_file_writer()?;
        let mut td_cursor =
            self.database_writer().tx_ref().cursor_read::<tables::HeaderTerminalDifficulties>()?;

        let first_td = if initial_block_number == 0 {
            U256::ZERO
        } else {
            td_cursor
                .seek_exact(initial_block_number - 1)?
                .map(|(_, td)| td.0)
                .ok_or_else(|| ProviderError::TotalDifficultyNotFound(initial_block_number))?
        };

        for pair in headers {
            let (header, hash) = pair.borrow();
            let header = header.borrow();
            let td = first_td + header.difficulty;
            self.static_file_writer().append_header(header, td, hash)?;
        }

        Ok(())
    }

    /// Appends transactions to static files, using the
    /// [`BlockBodyIndices`](tables::BlockBodyIndices) table to determine the transaction number
    /// when appending to static files.
    ///
    /// NOTE: The static file writer used to construct this [`StorageWriter`] MUST be a writer for
    /// the Transactions segment.
    pub fn append_transactions_from_blocks<T>(
        &mut self,
        initial_block_number: BlockNumber,
        transactions: impl Iterator<Item = T>,
    ) -> ProviderResult<()>
    where
        T: Borrow<Vec<TransactionSignedNoHash>>,
    {
        self.ensure_database_writer()?;
        self.ensure_static_file_writer()?;

        let mut bodies_cursor =
            self.database_writer().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        let mut last_tx_idx = None;
        for (idx, transactions) in transactions.enumerate() {
            let block_number = initial_block_number + idx as u64;

            let mut first_tx_index =
                bodies_cursor.seek_exact(block_number)?.map(|(_, indices)| indices.first_tx_num());

            // If there are no indices, that means there have been no transactions
            //
            // So instead of returning an error, use zero
            if block_number == initial_block_number && first_tx_index.is_none() {
                first_tx_index = Some(0);
            }

            let mut tx_index = first_tx_index
                .or(last_tx_idx)
                .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(block_number))?;

            for tx in transactions.borrow() {
                self.static_file_writer().append_transaction(tx_index, tx)?;
                tx_index += 1;
            }

            self.static_file_writer()
                .increment_block(StaticFileSegment::Transactions, block_number)?;

            // update index
            last_tx_idx = Some(tx_index);
        }
        Ok(())
    }
}

impl<'a, 'b, TX> StorageWriter<'a, 'b, TX>
where
    TX: DbTxMut + DbTx,
{
    /// Writes the hashed state changes to the database
    pub fn write_hashed_state(&self, hashed_state: &HashedPostStateSorted) -> ProviderResult<()> {
        self.ensure_database_writer()?;

        // Write hashed account updates.
        let mut hashed_accounts_cursor =
            self.database_writer().tx_ref().cursor_write::<tables::HashedAccounts>()?;
        for (hashed_address, account) in hashed_state.accounts().accounts_sorted() {
            if let Some(account) = account {
                hashed_accounts_cursor.upsert(hashed_address, account)?;
            } else if hashed_accounts_cursor.seek_exact(hashed_address)?.is_some() {
                hashed_accounts_cursor.delete_current()?;
            }
        }

        // Write hashed storage changes.
        let sorted_storages = hashed_state.account_storages().iter().sorted_by_key(|(key, _)| *key);
        let mut hashed_storage_cursor =
            self.database_writer().tx_ref().cursor_dup_write::<tables::HashedStorages>()?;
        for (hashed_address, storage) in sorted_storages {
            if storage.is_wiped() && hashed_storage_cursor.seek_exact(*hashed_address)?.is_some() {
                hashed_storage_cursor.delete_current_duplicates()?;
            }

            for (hashed_slot, value) in storage.storage_slots_sorted() {
                let entry = StorageEntry { key: hashed_slot, value };
                if let Some(db_entry) =
                    hashed_storage_cursor.seek_by_key_subkey(*hashed_address, entry.key)?
                {
                    if db_entry.key == entry.key {
                        hashed_storage_cursor.delete_current()?;
                    }
                }

                if !entry.value.is_zero() {
                    hashed_storage_cursor.upsert(*hashed_address, entry)?;
                }
            }
        }

        Ok(())
    }

    /// Appends receipts block by block.
    ///
    /// ATTENTION: If called from [`StorageWriter`] without a static file producer, it will always
    /// write them to database. Otherwise, it will look into the pruning configuration to decide.
    ///
    /// NOTE: The static file writer used to construct this [`StorageWriter`] MUST be a writer for
    /// the Receipts segment.
    ///
    /// # Parameters
    /// - `initial_block_number`: The starting block number.
    /// - `blocks`: An iterator over blocks, each block having a vector of optional receipts. If
    ///   `receipt` is `None`, it has been pruned.
    pub fn append_receipts_from_blocks(
        &mut self,
        initial_block_number: BlockNumber,
        blocks: impl Iterator<Item = Vec<Option<reth_primitives::Receipt>>>,
    ) -> ProviderResult<()> {
        self.ensure_database_writer()?;
        let mut bodies_cursor =
            self.database_writer().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        // We write receipts to database in two situations:
        // * If we are in live sync. In this case, `StorageWriter` is built without a static file
        //   writer.
        // * If there is any kind of receipt pruning
        let mut storage_type = if self.static_file_writer.is_none() ||
            self.database_writer().prune_modes_ref().has_receipts_pruning()
        {
            StorageType::Database(
                self.database_writer().tx_ref().cursor_write::<tables::Receipts>()?,
            )
        } else {
            self.ensure_static_file_writer()?;
            StorageType::StaticFile(self.static_file_writer())
        };

        let mut last_tx_idx = None;
        for (idx, receipts) in blocks.enumerate() {
            let block_number = initial_block_number + idx as u64;

            let mut first_tx_index =
                bodies_cursor.seek_exact(block_number)?.map(|(_, indices)| indices.first_tx_num());

            // If there are no indices, that means there have been no transactions
            //
            // So instead of returning an error, use zero
            if block_number == initial_block_number && first_tx_index.is_none() {
                first_tx_index = Some(0);
            }

            let first_tx_index = first_tx_index
                .or(last_tx_idx)
                .ok_or_else(|| ProviderError::BlockBodyIndicesNotFound(block_number))?;

            // update for empty blocks
            last_tx_idx = Some(first_tx_index);

            match &mut storage_type {
                StorageType::Database(cursor) => {
                    DatabaseWriter(cursor).append_block_receipts(
                        first_tx_index,
                        block_number,
                        receipts,
                    )?;
                }
                StorageType::StaticFile(sf) => {
                    StaticFileWriter(*sf).append_block_receipts(
                        first_tx_index,
                        block_number,
                        receipts,
                    )?;
                }
            };
        }

        Ok(())
    }

    /// Writes trie updates. Returns the number of entries modified.
    pub fn write_trie_updates(&self, trie_updates: &TrieUpdates) -> ProviderResult<usize> {
        self.ensure_database_writer()?;
        self.database_writer().write_trie_updates(trie_updates)
    }
}

impl<'a, 'b, TX> StateWriter for StorageWriter<'a, 'b, TX>
where
    TX: DbTxMut + DbTx,
{
    /// Write the data and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_to_storage(
        &mut self,
        execution_outcome: ExecutionOutcome,
        is_value_known: OriginalValuesKnown,
    ) -> ProviderResult<()> {
        self.ensure_database_writer()?;
        let (plain_state, reverts) =
            execution_outcome.bundle.into_plain_state_and_reverts(is_value_known);

        self.database_writer().write_state_reverts(reverts, execution_outcome.first_block)?;

        self.append_receipts_from_blocks(
            execution_outcome.first_block,
            execution_outcome.receipts.into_iter(),
        )?;

        self.database_writer().write_state_changes(plain_state)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{test_utils::create_test_provider_factory, AccountReader, TrieWriter};
    use reth_db::tables;
    use reth_db_api::{
        cursor::{DbCursorRO, DbDupCursorRO},
        models::{AccountBeforeTx, BlockNumberAddress},
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives::{
        keccak256, Account, Address, Receipt, Receipts, StorageEntry, B256, U256,
    };
    use reth_trie::{test_utils::state_root, HashedPostState, HashedStorage, StateRoot};
    use reth_trie_db::DatabaseStateRoot;
    use revm::{
        db::{
            states::{
                bundle_state::BundleRetention, changes::PlainStorageRevert, PlainStorageChangeset,
            },
            BundleState, EmptyDB,
        },
        primitives::{
            Account as RevmAccount, AccountInfo as RevmAccountInfo, AccountStatus, EvmStorageSlot,
        },
        DatabaseCommit, State,
    };
    use std::collections::{BTreeMap, HashMap};

    #[test]
    fn wiped_entries_are_removed() {
        let provider_factory = create_test_provider_factory();

        let addresses = (0..10).map(|_| Address::random()).collect::<Vec<_>>();
        let destroyed_address = *addresses.first().unwrap();
        let destroyed_address_hashed = keccak256(destroyed_address);
        let slot = B256::with_last_byte(1);
        let hashed_slot = keccak256(slot);
        {
            let provider_rw = provider_factory.provider_rw().unwrap();
            let mut accounts_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedAccounts>().unwrap();
            let mut storage_cursor =
                provider_rw.tx_ref().cursor_write::<tables::HashedStorages>().unwrap();

            for address in addresses {
                let hashed_address = keccak256(address);
                accounts_cursor
                    .insert(hashed_address, Account { nonce: 1, ..Default::default() })
                    .unwrap();
                storage_cursor
                    .insert(hashed_address, StorageEntry { key: hashed_slot, value: U256::from(1) })
                    .unwrap();
            }
            provider_rw.commit().unwrap();
        }

        let mut hashed_state = HashedPostState::default();
        hashed_state.accounts.insert(destroyed_address_hashed, None);
        hashed_state.storages.insert(destroyed_address_hashed, HashedStorage::new(true));

        let provider_rw = provider_factory.provider_rw().unwrap();
        let storage_writer = StorageWriter::new(Some(&provider_rw), None);
        assert_eq!(storage_writer.write_hashed_state(&hashed_state.into_sorted()), Ok(()));
        provider_rw.commit().unwrap();

        let provider = provider_factory.provider().unwrap();
        assert_eq!(
            provider.tx_ref().get::<tables::HashedAccounts>(destroyed_address_hashed),
            Ok(None)
        );
        assert_eq!(
            provider
                .tx_ref()
                .cursor_read::<tables::HashedStorages>()
                .unwrap()
                .seek_by_key_subkey(destroyed_address_hashed, hashed_slot),
            Ok(None)
        );
    }

    #[test]
    fn write_to_db_account_info() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_a = RevmAccountInfo { balance: U256::from(1), nonce: 1, ..Default::default() };
        let account_b = RevmAccountInfo { balance: U256::from(2), nonce: 2, ..Default::default() };
        let account_b_changed =
            RevmAccountInfo { balance: U256::from(3), nonce: 3, ..Default::default() };

        let mut state = State::builder().with_bundle_update().build();
        state.insert_not_existing(address_a);
        state.insert_account(address_b, account_b.clone());

        // 0x00.. is created
        state.commit(HashMap::from([(
            address_a,
            RevmAccount {
                info: account_a.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                storage: HashMap::default(),
            },
        )]));

        // 0xff.. is changed (balance + 1, nonce + 1)
        state.commit(HashMap::from([(
            address_b,
            RevmAccount {
                info: account_b_changed.clone(),
                status: AccountStatus::Touched,
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        let mut revm_bundle_state = state.take_bundle();

        // Write plain state and reverts separately.
        let reverts = revm_bundle_state.take_all_reverts().into_plain_state_reverts();
        let plain_state = revm_bundle_state.into_plain_state(OriginalValuesKnown::Yes);
        assert!(plain_state.storage.is_empty());
        assert!(plain_state.contracts.is_empty());
        provider.write_state_changes(plain_state).expect("Could not write plain state to DB");

        assert_eq!(reverts.storage, [[]]);
        provider.write_state_reverts(reverts, 1).expect("Could not write reverts to DB");

        let reth_account_a = account_a.into();
        let reth_account_b = account_b.into();
        let reth_account_b_changed = account_b_changed.clone().into();

        // Check plain state
        assert_eq!(
            provider.basic_account(address_a).expect("Could not read account state"),
            Some(reth_account_a),
            "Account A state is wrong"
        );
        assert_eq!(
            provider.basic_account(address_b).expect("Could not read account state"),
            Some(reth_account_b_changed),
            "Account B state is wrong"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::AccountChangeSets>()
            .expect("Could not open changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(1).expect("Could not read account change set"),
            Some((1, AccountBeforeTx { address: address_a, info: None })),
            "Account A changeset is wrong"
        );
        assert_eq!(
            changeset_cursor.next_dup().expect("Changeset table is malformed"),
            Some((1, AccountBeforeTx { address: address_b, info: Some(reth_account_b) })),
            "Account B changeset is wrong"
        );

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account(address_b, account_b_changed.clone());

        // 0xff.. is destroyed
        state.commit(HashMap::from([(
            address_b,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_b_changed,
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        let mut revm_bundle_state = state.take_bundle();

        // Write plain state and reverts separately.
        let reverts = revm_bundle_state.take_all_reverts().into_plain_state_reverts();
        let plain_state = revm_bundle_state.into_plain_state(OriginalValuesKnown::Yes);
        // Account B selfdestructed so flag for it should be present.
        assert_eq!(
            plain_state.storage,
            [PlainStorageChangeset { address: address_b, wipe_storage: true, storage: vec![] }]
        );
        assert!(plain_state.contracts.is_empty());
        provider.write_state_changes(plain_state).expect("Could not write plain state to DB");

        assert_eq!(
            reverts.storage,
            [[PlainStorageRevert { address: address_b, wiped: true, storage_revert: vec![] }]]
        );
        provider.write_state_reverts(reverts, 2).expect("Could not write reverts to DB");

        // Check new plain state for account B
        assert_eq!(
            provider.basic_account(address_b).expect("Could not read account state"),
            None,
            "Account B should be deleted"
        );

        // Check change set
        assert_eq!(
            changeset_cursor.seek_exact(2).expect("Could not read account change set"),
            Some((2, AccountBeforeTx { address: address_b, info: Some(reth_account_b_changed) })),
            "Account B changeset is wrong after deletion"
        );
    }

    #[test]
    fn write_to_db_storage() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_b = RevmAccountInfo { balance: U256::from(2), nonce: 2, ..Default::default() };

        let mut state = State::builder().with_bundle_update().build();
        state.insert_not_existing(address_a);
        state.insert_account_with_storage(
            address_b,
            account_b.clone(),
            HashMap::from([(U256::from(1), U256::from(1))]),
        );

        state.commit(HashMap::from([
            (
                address_a,
                RevmAccount {
                    status: AccountStatus::Touched | AccountStatus::Created,
                    info: RevmAccountInfo::default(),
                    // 0x00 => 0 => 1
                    // 0x01 => 0 => 2
                    storage: HashMap::from([
                        (
                            U256::from(0),
                            EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                        ),
                        (
                            U256::from(1),
                            EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                        ),
                    ]),
                },
            ),
            (
                address_b,
                RevmAccount {
                    status: AccountStatus::Touched,
                    info: account_b,
                    // 0x01 => 1 => 2
                    storage: HashMap::from([(
                        U256::from(1),
                        EvmStorageSlot {
                            present_value: U256::from(2),
                            original_value: U256::from(1),
                            ..Default::default()
                        },
                    )]),
                },
            ),
        ]));

        state.merge_transitions(BundleRetention::Reverts);

        let outcome =
            ExecutionOutcome::new(state.take_bundle(), Receipts::default(), 1, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        // Check plain storage state
        let mut storage_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::PlainStorageState>()
            .expect("Could not open plain storage state cursor");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            Some((address_a, StorageEntry { key: B256::ZERO, value: U256::from(1) })),
            "Slot 0 for account A should be 1"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            Some((
                address_a,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account A should only have 2 storage slots"
        );

        assert_eq!(
            storage_cursor.seek_exact(address_b).unwrap(),
            Some((
                address_b,
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account B should be 2"
        );
        assert_eq!(
            storage_cursor.next_dup().unwrap(),
            None,
            "Account B should only have 1 storage slot"
        );

        // Check change set
        let mut changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open storage changeset cursor");
        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_a))).unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(0) }
            )),
            "Slot 0 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((1, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(0) }
            )),
            "Slot 1 for account A should have changed from 0"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((1, address_b))).unwrap(),
            Some((
                BlockNumberAddress((1, address_b)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(1) }
            )),
            "Slot 1 for account B should have changed from 1"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account B should only be in the changeset 1 time"
        );

        // Delete account A
        let mut state = State::builder().with_bundle_update().build();
        state.insert_account(address_a, RevmAccountInfo::default());

        state.commit(HashMap::from([(
            address_a,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: RevmAccountInfo::default(),
                storage: HashMap::default(),
            },
        )]));

        state.merge_transitions(BundleRetention::Reverts);
        let outcome =
            ExecutionOutcome::new(state.take_bundle(), Receipts::default(), 2, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        assert_eq!(
            storage_cursor.seek_exact(address_a).unwrap(),
            None,
            "Account A should have no storage slots after deletion"
        );

        assert_eq!(
            changeset_cursor.seek_exact(BlockNumberAddress((2, address_a))).unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::ZERO, value: U256::from(1) }
            )),
            "Slot 0 for account A should have changed from 1 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            Some((
                BlockNumberAddress((2, address_a)),
                StorageEntry { key: B256::from(U256::from(1).to_be_bytes()), value: U256::from(2) }
            )),
            "Slot 1 for account A should have changed from 2 on deletion"
        );
        assert_eq!(
            changeset_cursor.next_dup().unwrap(),
            None,
            "Account A should only be in the changeset 2 times on deletion"
        );
    }

    #[test]
    fn write_to_db_multiple_selfdestructs() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::random();
        let account_info = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from([(
            address1,
            RevmAccount {
                info: account_info.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.merge_transitions(BundleRetention::Reverts);

        let outcome =
            ExecutionOutcome::new(init_state.take_bundle(), Receipts::default(), 0, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account_info.clone(),
            HashMap::from([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 1 => 2
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot {
                        original_value: U256::from(1),
                        present_value: U256::from(2),
                        ..Default::default()
                    },
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #2: destroy account.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #3: re-create account and change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #4: change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                // 0x02 => 0 => 4
                // 0x06 => 0 => 6
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                    (
                        U256::from(2),
                        EvmStorageSlot { present_value: U256::from(4), ..Default::default() },
                    ),
                    (
                        U256::from(6),
                        EvmStorageSlot { present_value: U256::from(6), ..Default::default() },
                    ),
                ]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #5: Destroy account again.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #6: Create, change, destroy and re-create in the same block.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                )]),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #7: Change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info,
                // 0x00 => 0 => 9
                storage: HashMap::from([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(9), ..Default::default() },
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        let bundle = state.take_bundle();

        let outcome = ExecutionOutcome::new(bundle, Receipts::default(), 1, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let mut storage_changes = storage_changeset_cursor.walk_range(..).unwrap();

        // Iterate through all storage changes

        // Block <number>
        // <slot>: <expected value before>
        // ...

        // Block #0
        // 0x00: 0
        // 0x01: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((0, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::ZERO }
            )))
        );

        // Block #1
        // 0x00: 1
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            )))
        );

        // Block #2 (destroyed)
        // 0x00: 2
        // 0x01: 2
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((2, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            )))
        );

        // Block #3
        // no storage changes

        // Block #4
        // 0x00: 0
        // 0x02: 0
        // 0x06: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::ZERO }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((4, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::ZERO }
            )))
        );

        // Block #5 (destroyed)
        // 0x00: 2
        // 0x02: 4
        // 0x06: 6
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(2) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(2), value: U256::from(4) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((5, address1)),
                StorageEntry { key: B256::with_last_byte(6), value: U256::from(6) }
            )))
        );

        // Block #6
        // no storage changes (only inter block changes)

        // Block #7
        // 0x00: 0
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((7, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::ZERO }
            )))
        );
        assert_eq!(storage_changes.next(), None);
    }

    #[test]
    fn storage_change_after_selfdestruct_within_block() {
        let factory = create_test_provider_factory();
        let provider = factory.provider_rw().unwrap();

        let address1 = Address::random();
        let account1 = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from([(
            address1,
            RevmAccount {
                info: account1.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from([
                    (
                        U256::ZERO,
                        EvmStorageSlot { present_value: U256::from(1), ..Default::default() },
                    ),
                    (
                        U256::from(1),
                        EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                    ),
                ]),
            },
        )]));
        init_state.merge_transitions(BundleRetention::Reverts);
        let outcome =
            ExecutionOutcome::new(init_state.take_bundle(), Receipts::default(), 0, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account1.clone(),
            HashMap::from([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: Destroy, re-create, change storage.
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account1,
                // 0x01 => 0 => 5
                storage: HashMap::from([(
                    U256::from(1),
                    EvmStorageSlot { present_value: U256::from(5), ..Default::default() },
                )]),
            },
        )]));

        // Commit block #1 changes to the database.
        state.merge_transitions(BundleRetention::Reverts);
        let outcome =
            ExecutionOutcome::new(state.take_bundle(), Receipts::default(), 1, Vec::new());
        let mut writer = StorageWriter::new(Some(&provider), None);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut storage_changeset_cursor = provider
            .tx_ref()
            .cursor_dup_read::<tables::StorageChangeSets>()
            .expect("Could not open plain storage state cursor");
        let range = BlockNumberAddress::range(1..=1);
        let mut storage_changes = storage_changeset_cursor.walk_range(range).unwrap();

        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(0), value: U256::from(1) }
            )))
        );
        assert_eq!(
            storage_changes.next(),
            Some(Ok((
                BlockNumberAddress((1, address1)),
                StorageEntry { key: B256::with_last_byte(1), value: U256::from(2) }
            )))
        );
        assert_eq!(storage_changes.next(), None);
    }

    #[test]
    fn revert_to_indices() {
        let base = ExecutionOutcome {
            bundle: BundleState::default(),
            receipts: vec![vec![Some(Receipt::default()); 2]; 7].into(),
            first_block: 10,
            requests: Vec::new(),
        };

        let mut this = base.clone();
        assert!(this.revert_to(10));
        assert_eq!(this.receipts.len(), 1);

        let mut this = base.clone();
        assert!(!this.revert_to(9));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base.clone();
        assert!(this.revert_to(15));
        assert_eq!(this.receipts.len(), 6);

        let mut this = base.clone();
        assert!(this.revert_to(16));
        assert_eq!(this.receipts.len(), 7);

        let mut this = base;
        assert!(!this.revert_to(17));
        assert_eq!(this.receipts.len(), 7);
    }

    #[test]
    fn bundle_state_state_root() {
        type PreState = BTreeMap<Address, (Account, BTreeMap<B256, U256>)>;
        let mut prestate: PreState = (0..10)
            .map(|key| {
                let account = Account { nonce: 1, balance: U256::from(key), bytecode_hash: None };
                let storage =
                    (1..11).map(|key| (B256::with_last_byte(key), U256::from(key))).collect();
                (Address::with_last_byte(key), (account, storage))
            })
            .collect();

        let provider_factory = create_test_provider_factory();
        let provider_rw = provider_factory.provider_rw().unwrap();

        // insert initial state to the database
        let tx = provider_rw.tx_ref();
        for (address, (account, storage)) in &prestate {
            let hashed_address = keccak256(address);
            tx.put::<tables::HashedAccounts>(hashed_address, *account).unwrap();
            for (slot, value) in storage {
                tx.put::<tables::HashedStorages>(
                    hashed_address,
                    StorageEntry { key: keccak256(slot), value: *value },
                )
                .unwrap();
            }
        }

        let (_, updates) = StateRoot::from_tx(tx).root_with_updates().unwrap();
        provider_rw.write_trie_updates(&updates).unwrap();

        let mut state = State::builder().with_bundle_update().build();

        let assert_state_root = |state: &State<EmptyDB>, expected: &PreState, msg| {
            assert_eq!(
                StateRoot::overlay_root(
                    tx,
                    ExecutionOutcome::new(
                        state.bundle_state.clone(),
                        Receipts::default(),
                        0,
                        Vec::new()
                    )
                    .hash_state_slow()
                )
                .unwrap(),
                state_root(expected.clone().into_iter().map(|(address, (account, storage))| (
                    address,
                    (account, storage.into_iter())
                ))),
                "{msg}"
            );
        };

        // database only state root is correct
        assert_state_root(&state, &prestate, "empty");

        // destroy account 1
        let address1 = Address::with_last_byte(1);
        let account1_old = prestate.remove(&address1).unwrap();
        state.insert_account(address1, account1_old.0.into());
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: RevmAccountInfo::default(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "destroyed account");

        // change slot 2 in account 2
        let address2 = Address::with_last_byte(2);
        let slot2 = U256::from(2);
        let slot2_key = B256::from(slot2);
        let account2 = prestate.get_mut(&address2).unwrap();
        let account2_slot2_old_value = *account2.1.get(&slot2_key).unwrap();
        state.insert_account_with_storage(
            address2,
            account2.0.into(),
            HashMap::from([(slot2, account2_slot2_old_value)]),
        );

        let account2_slot2_new_value = U256::from(100);
        account2.1.insert(slot2_key, account2_slot2_new_value);
        state.commit(HashMap::from([(
            address2,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account2.0.into(),
                storage: HashMap::from_iter([(
                    slot2,
                    EvmStorageSlot::new_changed(account2_slot2_old_value, account2_slot2_new_value),
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed storage");

        // change balance of account 3
        let address3 = Address::with_last_byte(3);
        let account3 = prestate.get_mut(&address3).unwrap();
        state.insert_account(address3, account3.0.into());

        account3.0.balance = U256::from(24);
        state.commit(HashMap::from([(
            address3,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account3.0.into(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed balance");

        // change nonce of account 4
        let address4 = Address::with_last_byte(4);
        let account4 = prestate.get_mut(&address4).unwrap();
        state.insert_account(address4, account4.0.into());

        account4.0.nonce = 128;
        state.commit(HashMap::from([(
            address4,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account4.0.into(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "changed nonce");

        // recreate account 1
        let account1_new =
            Account { nonce: 56, balance: U256::from(123), bytecode_hash: Some(B256::random()) };
        prestate.insert(address1, (account1_new, BTreeMap::default()));
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account1_new.into(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "recreated");

        // update storage for account 1
        let slot20 = U256::from(20);
        let slot20_key = B256::from(slot20);
        let account1_slot20_value = U256::from(12345);
        prestate.get_mut(&address1).unwrap().1.insert(slot20_key, account1_slot20_value);
        state.commit(HashMap::from([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account1_new.into(),
                storage: HashMap::from_iter([(
                    slot20,
                    EvmStorageSlot::new_changed(U256::ZERO, account1_slot20_value),
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::PlainState);
        assert_state_root(&state, &prestate, "recreated changed storage");
    }

    #[test]
    fn prepend_state() {
        let address1 = Address::random();
        let address2 = Address::random();

        let account1 = RevmAccountInfo { nonce: 1, ..Default::default() };
        let account1_changed = RevmAccountInfo { nonce: 1, ..Default::default() };
        let account2 = RevmAccountInfo { nonce: 1, ..Default::default() };

        let present_state = BundleState::builder(2..=2)
            .state_present_account_info(address1, account1_changed.clone())
            .build();
        assert_eq!(present_state.reverts.len(), 1);
        let previous_state = BundleState::builder(1..=1)
            .state_present_account_info(address1, account1)
            .state_present_account_info(address2, account2.clone())
            .build();
        assert_eq!(previous_state.reverts.len(), 1);

        let mut test = ExecutionOutcome {
            bundle: present_state,
            receipts: vec![vec![Some(Receipt::default()); 2]; 1].into(),
            first_block: 2,
            requests: Vec::new(),
        };

        test.prepend_state(previous_state);

        assert_eq!(test.receipts.len(), 1);
        let end_state = test.state();
        assert_eq!(end_state.state.len(), 2);
        // reverts num should stay the same.
        assert_eq!(end_state.reverts.len(), 1);
        // account1 is not overwritten.
        assert_eq!(end_state.state.get(&address1).unwrap().info, Some(account1_changed));
        // account2 got inserted
        assert_eq!(end_state.state.get(&address2).unwrap().info, Some(account2));
    }
}
