use crate::{
    providers::{StaticFileProvider, StaticFileProviderRWRefMut, StaticFileWriter as SfWriter},
    writer::static_file::StaticFileWriter,
    BlockExecutionWriter, BlockWriter, HistoryWriter, StateChangeWriter, StateWriter, TrieWriter,
};
use alloy_primitives::{BlockNumber, B256, U256};
use reth_chain_state::ExecutedBlock;
use reth_db::{
    cursor::DbCursorRO,
    models::CompactU256,
    tables,
    transaction::{DbTx, DbTxMut},
};
use reth_errors::{ProviderError, ProviderResult};
use reth_execution_types::ExecutionOutcome;
use reth_primitives::{Header, SealedBlock, StaticFileSegment, TransactionSignedNoHash};
use reth_stages_types::{StageCheckpoint, StageId};
use reth_storage_api::{
    DBProvider, HeaderProvider, ReceiptWriter, StageCheckpointWriter, TransactionsProviderExt,
};
use reth_storage_errors::writer::UnifiedStorageWriterError;
use revm::db::OriginalValuesKnown;
use std::{borrow::Borrow, sync::Arc};
use tracing::{debug, instrument};

mod database;
mod static_file;
use database::DatabaseWriter;

enum StorageType<C = (), S = ()> {
    Database(C),
    StaticFile(S),
}

/// [`UnifiedStorageWriter`] is responsible for managing the writing to storage with both database
/// and static file providers.
#[derive(Debug)]
pub struct UnifiedStorageWriter<'a, ProviderDB, ProviderSF> {
    database: &'a ProviderDB,
    static_file: Option<ProviderSF>,
}

impl<'a, ProviderDB, ProviderSF> UnifiedStorageWriter<'a, ProviderDB, ProviderSF> {
    /// Creates a new instance of [`UnifiedStorageWriter`].
    ///
    /// # Parameters
    /// - `database`: An optional reference to a database provider.
    /// - `static_file`: An optional mutable reference to a static file instance.
    pub const fn new(database: &'a ProviderDB, static_file: Option<ProviderSF>) -> Self {
        Self { database, static_file }
    }

    /// Creates a new instance of [`UnifiedStorageWriter`] from a database provider and a static
    /// file instance.
    pub fn from<P>(database: &'a P, static_file: ProviderSF) -> Self
    where
        P: AsRef<ProviderDB>,
    {
        Self::new(database.as_ref(), Some(static_file))
    }

    /// Creates a new instance of [`UnifiedStorageWriter`] from a database provider.
    pub fn from_database<P>(database: &'a P) -> Self
    where
        P: AsRef<ProviderDB>,
    {
        Self::new(database.as_ref(), None)
    }

    /// Returns a reference to the database writer.
    ///
    /// # Panics
    /// If the database provider is not set.
    const fn database(&self) -> &ProviderDB {
        self.database
    }

    /// Returns a reference to the static file instance.
    ///
    /// # Panics
    /// If the static file instance is not set.
    fn static_file(&self) -> &ProviderSF {
        self.static_file.as_ref().expect("should exist")
    }

    /// Returns a mutable reference to the static file instance.
    ///
    /// # Panics
    /// If the static file instance is not set.
    fn static_file_mut(&mut self) -> &mut ProviderSF {
        self.static_file.as_mut().expect("should exist")
    }

    /// Ensures that the static file instance is set.
    ///
    /// # Returns
    /// - `Ok(())` if the static file instance is set.
    /// - `Err(StorageWriterError::MissingStaticFileWriter)` if the static file instance is not set.
    #[allow(unused)]
    const fn ensure_static_file(&self) -> Result<(), UnifiedStorageWriterError> {
        if self.static_file.is_none() {
            return Err(UnifiedStorageWriterError::MissingStaticFileWriter)
        }
        Ok(())
    }
}

impl UnifiedStorageWriter<'_, (), ()> {
    /// Commits both storage types in the right order.
    ///
    /// For non-unwinding operations it makes more sense to commit the static files first, since if
    /// it is interrupted before the database commit, we can just truncate
    /// the static files according to the checkpoints on the next
    /// start-up.
    ///
    /// NOTE: If unwinding data from storage, use `commit_unwind` instead!
    pub fn commit<P>(
        database: impl Into<P> + AsRef<P>,
        static_file: StaticFileProvider,
    ) -> ProviderResult<()>
    where
        P: DBProvider<Tx: DbTxMut>,
    {
        static_file.commit()?;
        database.into().into_tx().commit()?;
        Ok(())
    }

    /// Commits both storage types in the right order for an unwind operation.
    ///
    /// For unwinding it makes more sense to commit the database first, since if
    /// it is interrupted before the static files commit, we can just
    /// truncate the static files according to the
    /// checkpoints on the next start-up.
    ///
    /// NOTE: Should only be used after unwinding data from storage!
    pub fn commit_unwind<P>(
        database: impl Into<P> + AsRef<P>,
        static_file: StaticFileProvider,
    ) -> ProviderResult<()>
    where
        P: DBProvider<Tx: DbTxMut>,
    {
        database.into().into_tx().commit()?;
        static_file.commit()?;
        Ok(())
    }
}

impl<ProviderDB> UnifiedStorageWriter<'_, ProviderDB, &StaticFileProvider>
where
    ProviderDB: DBProvider<Tx: DbTx + DbTxMut>
        + BlockWriter
        + TransactionsProviderExt
        + StateChangeWriter
        + TrieWriter
        + HistoryWriter
        + StageCheckpointWriter
        + BlockExecutionWriter
        + AsRef<ProviderDB>,
{
    /// Writes executed blocks and receipts to storage.
    pub fn save_blocks(&self, blocks: &[ExecutedBlock]) -> ProviderResult<()> {
        if blocks.is_empty() {
            debug!(target: "provider::storage_writer", "Attempted to write empty block range");
            return Ok(())
        }

        // NOTE: checked non-empty above
        let first_block = blocks.first().unwrap().block();
        let last_block = blocks.last().unwrap().block().clone();
        let first_number = first_block.number;
        let last_block_number = last_block.number;

        debug!(target: "provider::storage_writer", block_count = %blocks.len(), "Writing blocks and execution data to storage");

        // Only write receipts to static files if there is no receipt pruning configured.
        let mut state_writer = if self.database().prune_modes_ref().has_receipts_pruning() {
            UnifiedStorageWriter::from_database(self.database())
        } else {
            UnifiedStorageWriter::from(
                self.database(),
                self.static_file().get_writer(first_block.number, StaticFileSegment::Receipts)?,
            )
        };

        // TODO: remove all the clones and do performant / batched writes for each type of object
        // instead of a loop over all blocks,
        // meaning:
        //  * blocks
        //  * state
        //  * hashed state
        //  * trie updates (cannot naively extend, need helper)
        //  * indices (already done basically)
        // Insert the blocks
        for block in blocks {
            let sealed_block =
                block.block().clone().try_with_senders_unchecked(block.senders().clone()).unwrap();
            self.database().insert_block(sealed_block)?;
            self.save_header_and_transactions(block.block.clone())?;

            // Write state and changesets to the database.
            // Must be written after blocks because of the receipt lookup.
            let execution_outcome = block.execution_outcome().clone();
            state_writer.write_to_storage(execution_outcome, OriginalValuesKnown::No)?;

            // insert hashes and intermediate merkle nodes
            {
                let trie_updates = block.trie_updates().clone();
                let hashed_state = block.hashed_state();
                self.database().write_hashed_state(&hashed_state.clone().into_sorted())?;
                self.database().write_trie_updates(&trie_updates)?;
            }
        }

        // update history indices
        self.database().update_history_indices(first_number..=last_block_number)?;

        // Update pipeline progress
        self.database().update_pipeline_stages(last_block_number, false)?;

        debug!(target: "provider::storage_writer", range = ?first_number..=last_block_number, "Appended block data");

        Ok(())
    }

    /// Writes the header & transactions to static files, and updates their respective checkpoints
    /// on database.
    #[instrument(level = "trace", skip_all, fields(block = ?block.num_hash()) target = "storage")]
    fn save_header_and_transactions(&self, block: Arc<SealedBlock>) -> ProviderResult<()> {
        debug!(target: "provider::storage_writer", "Writing headers and transactions.");

        {
            let header_writer =
                self.static_file().get_writer(block.number, StaticFileSegment::Headers)?;
            let mut storage_writer = UnifiedStorageWriter::from(self.database(), header_writer);
            let td = storage_writer.append_headers_from_blocks(
                block.header().number,
                std::iter::once(&(block.header(), block.hash())),
            )?;

            debug!(target: "provider::storage_writer", block_num=block.number, "Updating transaction metadata after writing");
            self.database()
                .tx_ref()
                .put::<tables::HeaderTerminalDifficulties>(block.number, CompactU256(td))?;
            self.database()
                .save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(block.number))?;
        }

        {
            let transactions_writer =
                self.static_file().get_writer(block.number, StaticFileSegment::Transactions)?;
            let mut storage_writer =
                UnifiedStorageWriter::from(self.database(), transactions_writer);
            let no_hash_transactions = block
                .body
                .transactions
                .clone()
                .into_iter()
                .map(TransactionSignedNoHash::from)
                .collect();
            storage_writer.append_transactions_from_blocks(
                block.header().number,
                std::iter::once(&no_hash_transactions),
            )?;
            self.database()
                .save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(block.number))?;
        }

        Ok(())
    }

    /// Removes all block, transaction and receipt data above the given block number from the
    /// database and static files. This is exclusive, i.e., it only removes blocks above
    /// `block_number`, and does not remove `block_number`.
    pub fn remove_blocks_above(&self, block_number: u64) -> ProviderResult<()> {
        // Get highest static file block for the total block range
        let highest_static_file_block = self
            .static_file()
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .expect("todo: error handling, headers should exist");

        // Get the total txs for the block range, so we have the correct number of columns for
        // receipts and transactions
        // IMPORTANT: we use `block_number+1` to make sure we remove only what is ABOVE the block
        let tx_range = self
            .database()
            .transaction_range_by_block_range(block_number + 1..=highest_static_file_block)?;
        let total_txs = tx_range.end().saturating_sub(*tx_range.start());

        // IMPORTANT: we use `block_number+1` to make sure we remove only what is ABOVE the block
        debug!(target: "provider::storage_writer", ?block_number, "Removing blocks from database above block_number");
        self.database().remove_block_and_execution_range(
            block_number + 1..=self.database().last_block_number()?,
        )?;

        // IMPORTANT: we use `highest_static_file_block.saturating_sub(block_number)` to make sure
        // we remove only what is ABOVE the block.
        //
        // i.e., if the highest static file block is 8, we want to remove above block 5 only, we
        // will have three blocks to remove, which will be block 8, 7, and 6.
        debug!(target: "provider::storage_writer", ?block_number, "Removing static file blocks above block_number");
        self.static_file()
            .get_writer(block_number, StaticFileSegment::Headers)?
            .prune_headers(highest_static_file_block.saturating_sub(block_number))?;

        self.static_file()
            .get_writer(block_number, StaticFileSegment::Transactions)?
            .prune_transactions(total_txs, block_number)?;

        if !self.database().prune_modes_ref().has_receipts_pruning() {
            self.static_file()
                .get_writer(block_number, StaticFileSegment::Receipts)?
                .prune_receipts(total_txs, block_number)?;
        }

        Ok(())
    }
}

impl<ProviderDB> UnifiedStorageWriter<'_, ProviderDB, StaticFileProviderRWRefMut<'_>>
where
    ProviderDB: DBProvider<Tx: DbTx> + HeaderProvider,
{
    /// Ensures that the static file writer is set and of the right [`StaticFileSegment`] variant.
    ///
    /// # Returns
    /// - `Ok(())` if the static file writer is set.
    /// - `Err(StorageWriterError::MissingStaticFileWriter)` if the static file instance is not set.
    fn ensure_static_file_segment(
        &self,
        segment: StaticFileSegment,
    ) -> Result<(), UnifiedStorageWriterError> {
        match &self.static_file {
            Some(writer) => {
                if writer.user_header().segment() == segment {
                    Ok(())
                } else {
                    Err(UnifiedStorageWriterError::IncorrectStaticFileWriter(
                        writer.user_header().segment(),
                        segment,
                    ))
                }
            }
            None => Err(UnifiedStorageWriterError::MissingStaticFileWriter),
        }
    }

    /// Appends headers to static files, using the
    /// [`HeaderTerminalDifficulties`](tables::HeaderTerminalDifficulties) table to determine the
    /// total difficulty of the parent block during header insertion.
    ///
    /// NOTE: The static file writer used to construct this [`UnifiedStorageWriter`] MUST be a
    /// writer for the Headers segment.
    pub fn append_headers_from_blocks<H, I>(
        &mut self,
        initial_block_number: BlockNumber,
        headers: impl Iterator<Item = I>,
    ) -> ProviderResult<U256>
    where
        I: Borrow<(H, B256)>,
        H: Borrow<Header>,
    {
        self.ensure_static_file_segment(StaticFileSegment::Headers)?;

        let mut td = self
            .database()
            .header_td_by_number(initial_block_number)?
            .ok_or(ProviderError::TotalDifficultyNotFound(initial_block_number))?;

        for pair in headers {
            let (header, hash) = pair.borrow();
            let header = header.borrow();
            td += header.difficulty;
            self.static_file_mut().append_header(header, td, hash)?;
        }

        Ok(td)
    }

    /// Appends transactions to static files, using the
    /// [`BlockBodyIndices`](tables::BlockBodyIndices) table to determine the transaction number
    /// when appending to static files.
    ///
    /// NOTE: The static file writer used to construct this [`UnifiedStorageWriter`] MUST be a
    /// writer for the Transactions segment.
    pub fn append_transactions_from_blocks<T>(
        &mut self,
        initial_block_number: BlockNumber,
        transactions: impl Iterator<Item = T>,
    ) -> ProviderResult<()>
    where
        T: Borrow<Vec<TransactionSignedNoHash>>,
    {
        self.ensure_static_file_segment(StaticFileSegment::Transactions)?;

        let mut bodies_cursor =
            self.database().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

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
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block_number))?;

            for tx in transactions.borrow() {
                self.static_file_mut().append_transaction(tx_index, tx)?;
                tx_index += 1;
            }

            self.static_file_mut().increment_block(block_number)?;

            // update index
            last_tx_idx = Some(tx_index);
        }
        Ok(())
    }
}

impl<ProviderDB> UnifiedStorageWriter<'_, ProviderDB, StaticFileProviderRWRefMut<'_>>
where
    ProviderDB: DBProvider<Tx: DbTxMut + DbTx> + HeaderProvider,
{
    /// Appends receipts block by block.
    ///
    /// ATTENTION: If called from [`UnifiedStorageWriter`] without a static file producer, it will
    /// always write them to database. Otherwise, it will look into the pruning configuration to
    /// decide.
    ///
    /// NOTE: The static file writer used to construct this [`UnifiedStorageWriter`] MUST be a
    /// writer for the Receipts segment.
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
        let mut bodies_cursor =
            self.database().tx_ref().cursor_read::<tables::BlockBodyIndices>()?;

        // We write receipts to database in two situations:
        // * If we are in live sync. In this case, `UnifiedStorageWriter` is built without a static
        //   file writer.
        // * If there is any kind of receipt pruning
        let mut storage_type = if self.static_file.is_none() ||
            self.database().prune_modes_ref().has_receipts_pruning()
        {
            StorageType::Database(self.database().tx_ref().cursor_write::<tables::Receipts>()?)
        } else {
            self.ensure_static_file_segment(StaticFileSegment::Receipts)?;
            StorageType::StaticFile(self.static_file_mut())
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
                .ok_or(ProviderError::BlockBodyIndicesNotFound(block_number))?;

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
}

impl<ProviderDB> StateWriter
    for UnifiedStorageWriter<'_, ProviderDB, StaticFileProviderRWRefMut<'_>>
where
    ProviderDB: DBProvider<Tx: DbTxMut + DbTx> + StateChangeWriter + HeaderProvider,
{
    /// Write the data and receipts to the database or static files if `static_file_producer` is
    /// `Some`. It should be `None` if there is any kind of pruning/filtering over the receipts.
    fn write_to_storage(
        &mut self,
        execution_outcome: ExecutionOutcome,
        is_value_known: OriginalValuesKnown,
    ) -> ProviderResult<()> {
        let (plain_state, reverts) =
            execution_outcome.bundle.into_plain_state_and_reverts(is_value_known);

        self.database().write_state_reverts(reverts, execution_outcome.first_block)?;

        self.append_receipts_from_blocks(
            execution_outcome.first_block,
            execution_outcome.receipts.into_iter(),
        )?;

        self.database().write_state_changes(plain_state)?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        test_utils::create_test_provider_factory, AccountReader, StorageTrieWriter, TrieWriter,
    };
    use alloy_primitives::{keccak256, map::HashMap, Address, B256, U256};
    use reth_db::tables;
    use reth_db_api::{
        cursor::{DbCursorRO, DbCursorRW, DbDupCursorRO},
        models::{AccountBeforeTx, BlockNumberAddress},
        transaction::{DbTx, DbTxMut},
    };
    use reth_primitives::{Account, Receipt, Receipts, StorageEntry};
    use reth_storage_api::DatabaseProviderFactory;
    use reth_trie::{
        test_utils::{state_root, storage_root_prehashed},
        HashedPostState, HashedStorage, StateRoot, StorageRoot,
    };
    use reth_trie_db::{DatabaseStateRoot, DatabaseStorageRoot};
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
    use std::{collections::BTreeMap, str::FromStr};

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
        assert_eq!(provider_rw.write_hashed_state(&hashed_state.into_sorted()), Ok(()));
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
        state.commit(HashMap::from_iter([(
            address_a,
            RevmAccount {
                info: account_a.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                storage: HashMap::default(),
            },
        )]));

        // 0xff.. is changed (balance + 1, nonce + 1)
        state.commit(HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
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
        let provider = factory.database_provider_rw().unwrap();

        let address_a = Address::ZERO;
        let address_b = Address::repeat_byte(0xff);

        let account_b = RevmAccountInfo { balance: U256::from(2), nonce: 2, ..Default::default() };

        let mut state = State::builder().with_bundle_update().build();
        state.insert_not_existing(address_a);
        state.insert_account_with_storage(
            address_b,
            account_b.clone(),
            HashMap::from_iter([(U256::from(1), U256::from(1))]),
        );

        state.commit(HashMap::from_iter([
            (
                address_a,
                RevmAccount {
                    status: AccountStatus::Touched | AccountStatus::Created,
                    info: RevmAccountInfo::default(),
                    // 0x00 => 0 => 1
                    // 0x01 => 0 => 2
                    storage: HashMap::from_iter([
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
                    storage: HashMap::from_iter([(
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
        let mut writer = UnifiedStorageWriter::from_database(&provider);
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

        state.commit(HashMap::from_iter([(
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
        let mut writer = UnifiedStorageWriter::from_database(&provider);
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
        let provider = factory.database_provider_rw().unwrap();

        let address1 = Address::random();
        let account_info = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                info: account_info.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from_iter([
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
        let mut writer = UnifiedStorageWriter::from_database(&provider);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account_info.clone(),
            HashMap::from_iter([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: change storage.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 1 => 2
                storage: HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #3: re-create account and change storage.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #4: change storage.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                // 0x02 => 0 => 4
                // 0x06 => 0 => 6
                storage: HashMap::from_iter([
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
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #6: Create, change, destroy and re-create in the same block.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info.clone(),
                // 0x00 => 0 => 2
                storage: HashMap::from_iter([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(2), ..Default::default() },
                )]),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account_info.clone(),
                storage: HashMap::default(),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        // Block #7: Change storage.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account_info,
                // 0x00 => 0 => 9
                storage: HashMap::from_iter([(
                    U256::ZERO,
                    EvmStorageSlot { present_value: U256::from(9), ..Default::default() },
                )]),
            },
        )]));
        state.merge_transitions(BundleRetention::Reverts);

        let bundle = state.take_bundle();

        let outcome = ExecutionOutcome::new(bundle, Receipts::default(), 1, Vec::new());
        let mut writer = UnifiedStorageWriter::from_database(&provider);
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
        let provider = factory.database_provider_rw().unwrap();

        let address1 = Address::random();
        let account1 = RevmAccountInfo { nonce: 1, ..Default::default() };

        // Block #0: initial state.
        let mut init_state = State::builder().with_bundle_update().build();
        init_state.insert_not_existing(address1);
        init_state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                info: account1.clone(),
                status: AccountStatus::Touched | AccountStatus::Created,
                // 0x00 => 0 => 1
                // 0x01 => 0 => 2
                storage: HashMap::from_iter([
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
        let mut writer = UnifiedStorageWriter::from_database(&provider);
        writer
            .write_to_storage(outcome, OriginalValuesKnown::Yes)
            .expect("Could not write bundle state to DB");

        let mut state = State::builder().with_bundle_update().build();
        state.insert_account_with_storage(
            address1,
            account1.clone(),
            HashMap::from_iter([(U256::ZERO, U256::from(1)), (U256::from(1), U256::from(2))]),
        );

        // Block #1: Destroy, re-create, change storage.
        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::SelfDestructed,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched | AccountStatus::Created,
                info: account1.clone(),
                storage: HashMap::default(),
            },
        )]));

        state.commit(HashMap::from_iter([(
            address1,
            RevmAccount {
                status: AccountStatus::Touched,
                info: account1,
                // 0x01 => 0 => 5
                storage: HashMap::from_iter([(
                    U256::from(1),
                    EvmStorageSlot { present_value: U256::from(5), ..Default::default() },
                )]),
            },
        )]));

        // Commit block #1 changes to the database.
        state.merge_transitions(BundleRetention::Reverts);
        let outcome =
            ExecutionOutcome::new(state.take_bundle(), Receipts::default(), 1, Vec::new());
        let mut writer = UnifiedStorageWriter::from_database(&provider);
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
        let provider_rw = provider_factory.database_provider_rw().unwrap();

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
                    .hash_state_slow(),
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
        state.commit(HashMap::from_iter([(
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
            HashMap::from_iter([(slot2, account2_slot2_old_value)]),
        );

        let account2_slot2_new_value = U256::from(100);
        account2.1.insert(slot2_key, account2_slot2_new_value);
        state.commit(HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
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
        state.commit(HashMap::from_iter([(
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

    #[test]
    fn hashed_state_storage_root() {
        let address = Address::random();
        let hashed_address = keccak256(address);
        let provider_factory = create_test_provider_factory();
        let provider_rw = provider_factory.provider_rw().unwrap();
        let tx = provider_rw.tx_ref();

        // insert initial account storage
        let init_storage = HashedStorage::from_iter(
            false,
            [
                "50000000000000000000000000000004253371b55351a08cb3267d4d265530b6",
                "512428ed685fff57294d1a9cbb147b18ae5db9cf6ae4b312fa1946ba0561882e",
                "51e6784c736ef8548f856909870b38e49ef7a4e3e77e5e945e0d5e6fcaa3037f",
            ]
            .into_iter()
            .map(|str| (B256::from_str(str).unwrap(), U256::from(1))),
        );
        let mut state = HashedPostState::default();
        state.storages.insert(hashed_address, init_storage.clone());
        provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();

        // calculate database storage root and write intermediate storage nodes.
        let (storage_root, _, storage_updates) =
            StorageRoot::from_tx_hashed(tx, hashed_address).calculate(true).unwrap();
        assert_eq!(storage_root, storage_root_prehashed(init_storage.storage));
        assert!(!storage_updates.is_empty());
        provider_rw
            .write_individual_storage_trie_updates(hashed_address, &storage_updates)
            .unwrap();

        // destroy the storage and re-create with new slots
        let updated_storage = HashedStorage::from_iter(
            true,
            [
                "00deb8486ad8edccfdedfc07109b3667b38a03a8009271aac250cce062d90917",
                "88d233b7380bb1bcdc866f6871c94685848f54cf0ee033b1480310b4ddb75fc9",
            ]
            .into_iter()
            .map(|str| (B256::from_str(str).unwrap(), U256::from(1))),
        );
        let mut state = HashedPostState::default();
        state.storages.insert(hashed_address, updated_storage.clone());
        provider_rw.write_hashed_state(&state.clone().into_sorted()).unwrap();

        // re-calculate database storage root
        let storage_root = StorageRoot::overlay_root(tx, address, updated_storage.clone()).unwrap();
        assert_eq!(storage_root, storage_root_prehashed(updated_storage.storage));
    }
}
