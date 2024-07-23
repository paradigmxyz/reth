#![allow(dead_code)]

use reth_chain_state::ExecutedBlock;
use reth_db::Database;
use reth_errors::ProviderResult;
use reth_primitives::{SealedBlock, StaticFileSegment, TransactionSignedNoHash, B256};
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

/// Writes parts of reth's in memory tree state to the database and static files.
///
/// This is meant to be a spawned service that listens for various incoming persistence operations,
/// performing those actions on disk, and returning the result in a channel.
///
/// This should be spawned in its own thread with [`std::thread::spawn`], since this performs
/// blocking I/O operations in an endless loop.
#[derive(Debug)]
pub struct PersistenceService<DB> {
    /// The provider factory to use
    provider: ProviderFactory<DB>,
    /// Incoming requests
    incoming: Receiver<PersistenceAction>,
    /// The pruner
    pruner: Pruner<DB, ProviderFactory<DB>>,
}

impl<DB: Database> PersistenceService<DB> {
    /// Create a new persistence service
    pub const fn new(
        provider: ProviderFactory<DB>,
        incoming: Receiver<PersistenceAction>,
        pruner: Pruner<DB, ProviderFactory<DB>>,
    ) -> Self {
        Self { provider, incoming, pruner }
    }

    /// Writes the cloned tree state to database
    fn write(&self, blocks: &[ExecutedBlock]) -> ProviderResult<()> {
        debug!(target: "tree::persistence", "Writing blocks to database");
        let provider_rw = self.provider.provider_rw()?;

        if blocks.is_empty() {
            debug!(target: "tree::persistence", "Attempted to write empty block range");
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
            // TODO: do we provide a static file producer here?
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

        debug!(target: "tree::persistence", range = ?first_number..=last_block_number, "Appended block data");

        Ok(())
    }

    /// Removes block data above the given block number from the database.
    /// This is exclusive, i.e., it only removes blocks above `block_number`, and does not remove
    /// `block_number`.
    ///
    /// This will then send a command to the static file service, to remove the actual block data.
    fn remove_blocks_above(&self, block_number: u64) -> ProviderResult<()> {
        debug!(target: "tree::persistence", ?block_number, "Removing blocks from database above block_number");
        let provider_rw = self.provider.provider_rw()?;
        let highest_block = self.provider.last_block_number()?;
        provider_rw.remove_block_and_execution_range(block_number..=highest_block)?;

        Ok(())
    }

    /// Prunes block data before the given block hash according to the configured prune
    /// configuration.
    fn prune_before(&mut self, block_num: u64) -> PrunerOutput {
        debug!(target: "tree::persistence", ?block_num, "Running pruner");
        // TODO: doing this properly depends on pruner segment changes
        self.pruner.run(block_num).expect("todo: handle errors")
    }

    /// Updates checkpoints related to block headers and bodies. This should be called after new
    /// transactions have been successfully written to disk.
    fn update_transaction_meta(&self, block_num: u64) -> ProviderResult<()> {
        debug!(target: "tree::persistence", ?block_num, "Updating transaction metadata after writing");
        let provider_rw = self.provider.provider_rw()?;
        provider_rw.save_stage_checkpoint(StageId::Headers, StageCheckpoint::new(block_num))?;
        provider_rw.save_stage_checkpoint(StageId::Bodies, StageCheckpoint::new(block_num))?;
        provider_rw.commit()?;
        Ok(())
    }

    /// Writes the transactions to static files.
    ///
    /// The [`update_transaction_meta`](Self::update_transaction_meta) method should be called
    /// after this, to update the checkpoints for headers and block bodies.
    fn write_transactions(&self, block: Arc<SealedBlock>) -> ProviderResult<u64> {
        debug!(target: "tree::persistence", "Writing transactions");
        let provider = self.provider.static_file_provider();

        let header_writer = provider.get_writer(block.number, StaticFileSegment::Headers)?;
        let provider_rw = self.provider.provider_rw()?;
        let mut storage_writer = StorageWriter::new(Some(&provider_rw), Some(header_writer));
        storage_writer.append_headers_from_blocks(
            block.header().number,
            std::iter::once(&(block.header(), block.hash())),
        )?;

        let transactions_writer =
            provider.get_writer(block.number, StaticFileSegment::Transactions)?;
        let mut storage_writer = StorageWriter::new(Some(&provider_rw), Some(transactions_writer));
        let no_hash_transactions =
            block.body.clone().into_iter().map(TransactionSignedNoHash::from).collect();
        storage_writer.append_transactions_from_blocks(
            block.header().number,
            std::iter::once(&no_hash_transactions),
        )?;

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
        debug!(target: "tree::persistence", len=blocks.len(), ?current_block, "Writing execution data to static files");

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
    fn remove_static_file_blocks_above(&self, block_number: u64) -> ProviderResult<()> {
        debug!(target: "tree::persistence", ?block_number, "Removing static file blocks above block_number");
        let sf_provider = self.provider.static_file_provider();
        let db_provider_rw = self.provider.provider_rw()?;

        // get highest static file block for the total block range
        let highest_static_file_block = sf_provider
            .get_highest_static_file_block(StaticFileSegment::Headers)
            .expect("todo: error handling, headers should exist");

        // Get the total txs for the block range, so we have the correct number of columns for
        // receipts and transactions
        let tx_range = db_provider_rw
            .transaction_range_by_block_range(block_number..=highest_static_file_block)?;
        let total_txs = tx_range.end().saturating_sub(*tx_range.start());

        // get the writers
        let mut header_writer = sf_provider.get_writer(block_number, StaticFileSegment::Headers)?;
        let mut transactions_writer =
            sf_provider.get_writer(block_number, StaticFileSegment::Transactions)?;
        let mut receipts_writer =
            sf_provider.get_writer(block_number, StaticFileSegment::Receipts)?;

        // finally actually truncate, these internally commit
        receipts_writer.prune_receipts(total_txs, block_number)?;
        transactions_writer.prune_transactions(total_txs, block_number)?;
        header_writer.prune_headers(highest_static_file_block.saturating_sub(block_number))?;

        sf_provider.commit()?;

        Ok(())
    }
}

impl<DB> PersistenceService<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to database events and perform the requested
    /// database actions
    pub fn run(mut self) {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::RemoveBlocksAbove((new_tip_num, sender)) => {
                    self.remove_blocks_above(new_tip_num).expect("todo: handle errors");
                    self.remove_static_file_blocks_above(new_tip_num).expect("todo: handle errors");

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(());
                }
                PersistenceAction::SaveBlocks((blocks, sender)) => {
                    if blocks.is_empty() {
                        todo!("return error or something");
                    }
                    let last_block_hash = blocks.last().unwrap().block().hash();
                    // first write to static files
                    self.write_execution_data(&blocks).expect("todo: handle errors");
                    // then write to db
                    self.write(&blocks).expect("todo: handle errors");

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(last_block_hash);
                }
                PersistenceAction::PruneBefore((block_num, sender)) => {
                    let res = self.prune_before(block_num);

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(res);
                }
                PersistenceAction::WriteTransactions((block, sender)) => {
                    let block_num = self.write_transactions(block).expect("todo: handle errors");
                    self.update_transaction_meta(block_num).expect("todo: handle errors");

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(());
                }
            }
        }
    }
}

/// A signal to the persistence service that part of the tree state can be persisted.
#[derive(Debug)]
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    ///
    /// First, header, transaction, and receipt-related data should be written to static files.
    /// Then the execution history-related data will be written to the database.
    SaveBlocks((Vec<ExecutedBlock>, oneshot::Sender<B256>)),

    /// The given block has been added to the canonical chain, its transactions and headers will be
    /// persisted for durability.
    ///
    /// This will first append the header and transactions to static files, then update the
    /// checkpoints for headers and block bodies in the database.
    WriteTransactions((Arc<SealedBlock>, oneshot::Sender<()>)),

    /// Removes block data above the given block number from the database.
    ///
    /// This will first update checkpoints from the database, then remove actual block data from
    /// static files.
    RemoveBlocksAbove((u64, oneshot::Sender<()>)),

    /// Prune associated block data before the given block number, according to already-configured
    /// prune modes.
    PruneBefore((u64, oneshot::Sender<PrunerOutput>)),
}

/// A handle to the persistence service
#[derive(Debug, Clone)]
pub struct PersistenceHandle {
    /// The channel used to communicate with the persistence service
    sender: Sender<PersistenceAction>,
}

impl PersistenceHandle {
    /// Create a new [`PersistenceHandle`] from a [`Sender<PersistenceAction>`].
    pub const fn new(sender: Sender<PersistenceAction>) -> Self {
        Self { sender }
    }

    /// Create a new [`PersistenceHandle`], and spawn the persistence service.
    pub fn spawn_services<DB: Database + 'static>(
        provider_factory: ProviderFactory<DB>,
        pruner: Pruner<DB, ProviderFactory<DB>>,
    ) -> Self {
        // create the initial channels
        let (db_service_tx, db_service_rx) = std::sync::mpsc::channel();

        // construct persistence handle
        let persistence_handle = Self::new(db_service_tx);

        // spawn the persistence service
        let db_service = PersistenceService::new(provider_factory, db_service_rx, pruner);
        std::thread::Builder::new()
            .name("Persistence Service".to_string())
            .spawn(|| db_service.run())
            .unwrap();

        persistence_handle
    }

    /// Sends a specific [`PersistenceAction`] in the contained channel. The caller is responsible
    /// for creating any channels for the given action.
    pub fn send_action(
        &self,
        action: PersistenceAction,
    ) -> Result<(), SendError<PersistenceAction>> {
        self.sender.send(action)
    }

    /// Tells the persistence service to save a certain list of finalized blocks. The blocks are
    /// assumed to be ordered by block number.
    ///
    /// This returns the latest hash that has been saved, allowing removal of that block and any
    /// previous blocks from in-memory data structures. This value is returned in the receiver end
    /// of the sender argument.
    pub fn save_blocks(&self, blocks: Vec<ExecutedBlock>, tx: oneshot::Sender<B256>) {
        if blocks.is_empty() {
            let _ = tx.send(B256::default());
            return;
        }
        self.send_action(PersistenceAction::SaveBlocks((blocks, tx)))
            .expect("should be able to send");
    }

    /// Tells the persistence service to remove blocks above a certain block number. The removed
    /// blocks are returned by the service.
    pub async fn remove_blocks_above(&self, block_num: u64) {
        let (tx, rx) = oneshot::channel();
        self.send_action(PersistenceAction::RemoveBlocksAbove((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the persistence service to remove block data before the given hash, according to the
    /// configured prune config.
    pub async fn prune_before(&self, block_num: u64) -> PrunerOutput {
        let (tx, rx) = oneshot::channel();
        self.send_action(PersistenceAction::PruneBefore((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chain_state::test_utils::{get_executed_block_with_number, get_executed_blocks};
    use reth_exex_types::FinishedExExHeight;
    use reth_primitives::B256;
    use reth_provider::{test_utils::create_test_provider_factory, ProviderFactory};
    use reth_prune::Pruner;

    fn default_persistence_handle() -> PersistenceHandle {
        let provider = create_test_provider_factory();

        let (finished_exex_height_tx, finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let pruner = Pruner::<_, ProviderFactory<_>>::new(
            provider.clone(),
            vec![],
            5,
            0,
            None,
            finished_exex_height_rx,
        );

        PersistenceHandle::spawn_services(provider, pruner)
    }

    #[tokio::test]
    async fn test_save_blocks_empty() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let blocks = vec![];
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx);

        let hash = rx.await.unwrap();
        assert_eq!(hash, B256::default());
    }

    #[tokio::test]
    async fn test_save_blocks_single_block() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();
        let block_number = 0;
        let executed = get_executed_block_with_number(block_number);
        let block_hash = executed.block().hash();

        let blocks = vec![executed];
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx);

        let actual_hash = rx.await.unwrap();
        assert_eq!(block_hash, actual_hash);
    }

    #[tokio::test]
    async fn test_save_blocks_multiple_blocks() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let blocks = get_executed_blocks(0..5).collect::<Vec<_>>();
        let last_hash = blocks.last().unwrap().block().hash();
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx);

        let actual_hash = rx.await.unwrap();
        assert_eq!(last_hash, actual_hash);
    }

    #[tokio::test]
    async fn test_save_blocks_multiple_calls() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let ranges = [0..1, 1..2, 2..4, 4..5];
        for range in ranges {
            let blocks = get_executed_blocks(range).collect::<Vec<_>>();
            let last_hash = blocks.last().unwrap().block().hash();
            let (tx, rx) = oneshot::channel();

            persistence_handle.save_blocks(blocks, tx);

            let actual_hash = rx.await.unwrap();
            assert_eq!(last_hash, actual_hash);
        }
    }
}
