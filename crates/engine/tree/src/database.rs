#![allow(dead_code)]

use crate::static_files::{StaticFileAction, StaticFileServiceHandle};
use reth_chain_state::ExecutedBlock;
use reth_db::database::Database;
use reth_errors::ProviderResult;
use reth_primitives::{SealedBlock, StaticFileSegment, TransactionSignedNoHash, B256, U256};
use reth_provider::{
    writer::StorageWriter, BlockExecutionWriter, BlockNumReader, BlockWriter, HistoryWriter,
    OriginalValuesKnown, ProviderFactory, StageCheckpointWriter, StateWriter,
    StaticFileProviderFactory, StaticFileWriter, TransactionsProviderExt,
};
use reth_prune::{Pruner, PrunerOutput};
use reth_stages_types::{StageCheckpoint, StageId};
use std::sync::{
    mpsc::{Receiver, SendError, Sender},
    Arc,
};
use tokio::sync::oneshot;
use tracing::debug;

/// Writes parts of reth's in memory tree state to the database.
///
/// This is meant to be a spawned service that listens for various incoming database operations,
/// performing those actions on disk, and returning the result in a channel.
///
/// There are two types of operations this service can perform:
/// - Writing executed blocks to disk, returning the hash of the latest block that was inserted.
/// - Removing blocks from disk, returning the hash of the lowest block removed.
///
/// This should be spawned in its own thread with [`std::thread::spawn`], since this performs
/// blocking database operations in an endless loop.
#[derive(Debug)]
pub struct DatabaseService<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    /// Incoming requests to persist stuff
    incoming: Receiver<DatabaseAction>,
    /// Handle for the static file service.
    static_file_handle: StaticFileServiceHandle,
    /// The pruner
    pruner: Pruner<DB, ProviderFactory<DB>>,
}

impl<DB: Database> DatabaseService<DB> {
    /// Create a new database service.
    ///
    /// NOTE: The [`ProviderFactory`] and [`Pruner`] should be built using an identical
    /// [`PruneModes`](reth_prune::PruneModes).
    pub const fn new(
        provider: ProviderFactory<DB>,
        incoming: Receiver<DatabaseAction>,
        static_file_handle: StaticFileServiceHandle,
        pruner: Pruner<DB, ProviderFactory<DB>>,
    ) -> Self {
        Self { provider, incoming, static_file_handle, pruner }
    }

    /// Writes the cloned tree state to the database
    fn write(&self, blocks: Vec<ExecutedBlock>) -> ProviderResult<()> {
        let provider_rw = self.provider.provider_rw()?;

        if blocks.is_empty() {
            debug!(target: "tree::persistence::db", "Attempted to write empty block range");
            return Ok(())
        }

        let first_number = blocks.first().unwrap().block().number;

        let last = blocks.last().unwrap().block();
        let last_block_number = last.number;

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
            provider_rw.insert_block(sealed_block)?;

            // Write state and changesets to the database.
            // Must be written after blocks because of the receipt lookup.
            let execution_outcome = block.execution_outcome().clone();
            // TODO: use single storage writer in task when sf / db tasks are combined
            execution_outcome.write_to_storage(&provider_rw, None, OriginalValuesKnown::No)?;

            // insert hashes and intermediate merkle nodes
            {
                let trie_updates = block.trie_updates().clone();
                let hashed_state = block.hashed_state();
                // TODO: use single storage writer in task when sf / db tasks are combined
                let storage_writer = StorageWriter::new(Some(&provider_rw), None);
                storage_writer.write_hashed_state(&hashed_state.clone().into_sorted())?;
                trie_updates.write_to_database(provider_rw.tx_ref())?;
            }

            // update history indices
            provider_rw.update_history_indices(first_number..=last_block_number)?;

            // Update pipeline progress
            provider_rw.update_pipeline_stages(last_block_number, false)?;
        }

        debug!(target: "tree::persistence::db", range = ?first_number..=last_block_number, "Appended blocks");

        Ok(())
    }

    /// Removes block data above the given block number from the database.
    /// This is exclusive, i.e., it only removes blocks above `block_number`, and does not remove
    /// `block_number`.
    ///
    /// This will then send a command to the static file service, to remove the actual block data.
    fn remove_blocks_above(
        &self,
        block_number: u64,
        sender: oneshot::Sender<()>,
    ) -> ProviderResult<()> {
        let provider_rw = self.provider.provider_rw()?;
        let highest_block = self.provider.last_block_number()?;
        provider_rw.remove_block_and_execution_range(block_number..=highest_block)?;

        // send a command to the static file service to also remove blocks
        let _ = self
            .static_file_handle
            .send_action(StaticFileAction::RemoveBlocksAbove((block_number, sender)));
        Ok(())
    }

    /// Prunes block data before the given block hash according to the configured prune
    /// configuration.
    fn prune_before(&mut self, block_num: u64) -> PrunerOutput {
        // TODO: doing this properly depends on pruner segment changes
        self.pruner.run(block_num).expect("todo: handle errors")
    }

    /// Updates checkpoints related to block headers and bodies. This should be called by the static
    /// file service, after new transactions have been successfully written to disk.
    fn update_transaction_meta(&self, block_num: u64) -> ProviderResult<()> {
        let provider_rw = self.provider.provider_rw()?;
        provider_rw.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(block_num))?;
        provider_rw.save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(block_num))?;
        provider_rw.commit()?;
        Ok(())
    }

    // TODO: some things about this are a bit weird, and just to make the underlying static file
    // writes work - tx number, total difficulty inclusion. They require either additional in memory
    // data or a db lookup. Maybe we can use a db read here
    /// Writes the transactions to static files, to act as a log.
    ///
    /// This will then send a command to the db service, that it should update the checkpoints for
    /// headers and block bodies.
    fn log_transactions(
        &self,
        block: Arc<SealedBlock>,
        start_tx_number: u64,
        td: U256,
    ) -> ProviderResult<u64> {
        let provider = self.provider.static_file_provider();
        let mut header_writer = provider.get_writer(block.number, StaticFileSegment::Headers)?;
        let mut transactions_writer =
            provider.get_writer(block.number, StaticFileSegment::Transactions)?;

        header_writer.append_header(block.header().clone(), td, block.hash())?;
        let no_hash_transactions =
            block.body.clone().into_iter().map(TransactionSignedNoHash::from);

        let mut tx_number = start_tx_number;
        for tx in no_hash_transactions {
            transactions_writer.append_transaction(tx_number, tx)?;
            tx_number += 1;
        }

        // increment block for transactions
        transactions_writer.increment_block(StaticFileSegment::Transactions, block.number)?;

        // finally commit
        transactions_writer.commit()?;
        header_writer.commit()?;

        Ok(block.number)
    }

    /// Write execution-related block data to static files.
    ///
    /// This will then send a command to the db service, that it should write new data, and update
    /// the checkpoints for execution and beyond.
    fn write_execution_data(&self, blocks: &[ExecutedBlock]) -> ProviderResult<()> {
        if blocks.is_empty() {
            return Ok(())
        }
        let provider_rw = self.provider.provider_rw()?;
        let provider = self.provider.static_file_provider();

        // NOTE: checked non-empty above
        let first_block = blocks.first().unwrap().block();
        let last_block = blocks.last().unwrap().block().clone();

        // use the storage writer
        let current_block = first_block.number;
        let receipts_writer =
            provider.get_writer(first_block.number, StaticFileSegment::Receipts)?;

        let storage_writer = StorageWriter::new(Some(&provider_rw), Some(receipts_writer));
        let receipts_iter = blocks.iter().map(|block| {
            let receipts = block.execution_outcome().receipts().receipt_vec.clone();
            debug_assert!(receipts.len() == 1);
            receipts.first().unwrap().clone()
        });
        storage_writer.append_receipts_from_blocks(current_block, receipts_iter)?;

        Ok(())
    }

    /// Removes the blocks above the given block number from static files. Also removes related
    /// receipt and header data.
    ///
    /// This is exclusive, i.e., it only removes blocks above `block_number`, and does not remove
    /// `block_number`.
    ///
    /// Returns the block hash for the lowest block removed from the database, which should be
    /// the hash for `block_number + 1`.
    ///
    /// This is meant to be called by the db service, as this should only be done after related data
    /// is removed from the database, and checkpoints are updated.
    ///
    /// Returns the hash of the lowest removed block.
    fn remove_static_file_blocks_above(
        &self,
        block_num: u64,
        sender: oneshot::Sender<()>,
    ) -> ProviderResult<()> {
        let sf_provider = self.provider.static_file_provider();
        let db_provider_rw = self.provider.provider_rw()?;

        // get highest static file block for the total block range
        let highest_static_file_block = sf_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .expect("todo: error handling, headers should exist");

        // Get the total txs for the block range, so we have the correct number of columns for
        // receipts and transactions
        let tx_range = db_provider_rw
            .transaction_range_by_block_range(block_num..=highest_static_file_block)?;
        let total_txs = tx_range.end().saturating_sub(*tx_range.start());

        // get the writers
        let mut header_writer = sf_provider.get_writer(block_num, StaticFileSegment::Headers)?;
        let mut transactions_writer =
            sf_provider.get_writer(block_num, StaticFileSegment::Transactions)?;
        let mut receipts_writer = sf_provider.get_writer(block_num, StaticFileSegment::Receipts)?;

        // finally actually truncate, these internally commit
        receipts_writer.prune_receipts(total_txs, block_num)?;
        transactions_writer.prune_transactions(total_txs, block_num)?;
        header_writer.prune_headers(highest_static_file_block.saturating_sub(block_num))?;

        sf_provider.commit()?;

        Ok(())
    }
}

impl<DB> DatabaseService<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to database events and perform the requested
    /// database actions
    pub fn run(mut self) {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                DatabaseAction::RemoveBlocksAbove((new_tip_num, sender)) => {
                    self.remove_blocks_above(new_tip_num, sender).expect("todo: handle errors");
                }
                DatabaseAction::SaveBlocks((blocks, sender)) => {
                    if blocks.is_empty() {
                        todo!("return error or something");
                    }
                    let last_block_hash = blocks.last().unwrap().block().hash();
                    // first write to db
                    self.write_execution_data(&blocks).expect("todo: handle errors");
                    // then write to static files
                    self.write(blocks).expect("todo: handle errors");

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(last_block_hash);
                }
                DatabaseAction::PruneBefore((block_num, sender)) => {
                    let res = self.prune_before(block_num);

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(res);
                }
                DatabaseAction::LogTransactions((block, start_tx_number, td, sender)) => {
                    let block_num = self
                        .log_transactions(block, start_tx_number, td)
                        .expect("todo: handle errors");
                    self.update_transaction_meta(block_num).expect("todo: handle errors");

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(());
                }
            }
        }
    }
}

/// A signal to the database service that part of the tree state can be persisted.
#[derive(Debug)]
pub enum DatabaseAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    ///
    /// This should just store the execution history-related data. Header, transaction, and
    /// receipt-related data should already be written to static files.
    SaveBlocks((Vec<ExecutedBlock>, oneshot::Sender<B256>)),

    /// The given block has been added to the canonical chain, its transactions and headers will be
    /// persisted for durability.
    ///
    /// This will then send a command to the db service, that it should update the checkpoints for
    /// headers and block bodies.
    LogTransactions((Arc<SealedBlock>, u64, U256, oneshot::Sender<()>)),

    /// Removes block data above the given block number from the database.
    ///
    /// This will then send a command to the static file service, to remove the actual block data.
    RemoveBlocksAbove((u64, oneshot::Sender<()>)),

    /// Prune associated block data before the given block number, according to already-configured
    /// prune modes.
    PruneBefore((u64, oneshot::Sender<PrunerOutput>)),
}

/// A handle to the database service
#[derive(Debug, Clone)]
pub struct DatabaseServiceHandle {
    /// The channel used to communicate with the database service
    sender: Sender<DatabaseAction>,
}

impl DatabaseServiceHandle {
    /// Create a new [`DatabaseServiceHandle`] from a [`Sender<DatabaseAction>`].
    pub const fn new(sender: Sender<DatabaseAction>) -> Self {
        Self { sender }
    }

    /// Sends a specific [`DatabaseAction`] in the contained channel. The caller is responsible
    /// for creating any channels for the given action.
    pub fn send_action(&self, action: DatabaseAction) -> Result<(), SendError<DatabaseAction>> {
        self.sender.send(action)
    }

    /// Tells the database service to save a certain list of finalized blocks. The blocks are
    /// assumed to be ordered by block number.
    ///
    /// This returns the latest hash that has been saved, allowing removal of that block and any
    /// previous blocks from in-memory data structures.
    pub async fn save_blocks(&self, blocks: Vec<ExecutedBlock>) -> B256 {
        let (tx, rx) = oneshot::channel();
        self.sender.send(DatabaseAction::SaveBlocks((blocks, tx))).expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the database service to remove blocks above a certain block number.
    pub async fn remove_blocks_above(&self, block_num: u64) {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(DatabaseAction::RemoveBlocksAbove((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the database service to remove block data before the given hash, according to the
    /// configured prune config.
    pub async fn prune_before(&self, block_num: u64) -> PrunerOutput {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(DatabaseAction::PruneBefore((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }
}
