#![allow(dead_code)]

use crate::tree::ExecutedBlock;
use reth_db::database::Database;
use reth_errors::ProviderResult;
use reth_primitives::B256;
use reth_provider::{
    bundle_state::HashedStateChanges, BlockWriter, HistoryWriter, OriginalValuesKnown,
    ProviderFactory, StageCheckpointWriter, StateWriter,
};
use reth_prune::{PruneProgress, Pruner};
use reth_trie_db::TxRefWrapper;
use std::sync::mpsc::{Receiver, SendError, Sender};
use tokio::sync::oneshot;
use tracing::debug;

/// Writes parts of reth's in memory tree state to the database.
///
/// This is meant to be a spawned task that listens for various incoming persistence operations,
/// performing those actions on disk, and returning the result in a channel.
///
/// There are two types of operations this task can perform:
/// - Writing executed blocks to disk, returning the hash of the latest block that was inserted.
/// - Removing blocks from disk, returning the removed blocks.
///
/// This should be spawned in its own thread with [`std::thread::spawn`], since this performs
/// blocking database operations in an endless loop.
#[derive(Debug)]
pub struct Persistence<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    /// Incoming requests to persist stuff
    incoming: Receiver<PersistenceAction>,
    /// The pruner
    pruner: Pruner<DB>,
}

impl<DB: Database> Persistence<DB> {
    /// Create a new persistence task
    const fn new(
        provider: ProviderFactory<DB>,
        incoming: Receiver<PersistenceAction>,
        pruner: Pruner<DB>,
    ) -> Self {
        Self { provider, incoming, pruner }
    }

    /// Writes the cloned tree state to the database
    fn write(&self, blocks: Vec<ExecutedBlock>) -> ProviderResult<()> {
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
            execution_outcome.write_to_storage(
                provider_rw.tx_ref(),
                None,
                OriginalValuesKnown::No,
            )?;

            // insert hashes and intermediate merkle nodes
            {
                let trie_updates = block.trie_updates().clone();
                let hashed_state = block.hashed_state();
                HashedStateChanges(hashed_state.clone()).write_to_db(provider_rw.tx_ref())?;
                trie_updates.write_to_database(&TxRefWrapper::from(provider_rw.tx_ref()))?;
            }

            // update history indices
            provider_rw.update_history_indices(first_number..=last_block_number)?;

            // Update pipeline progress
            provider_rw.update_pipeline_stages(last_block_number, false)?;
        }

        debug!(target: "tree::persistence", range = ?first_number..=last_block_number, "Appended blocks");

        Ok(())
    }

    /// Removes the blocks above the give block number from the database, returning them.
    fn remove_blocks_above(&self, _block_number: u64) -> Vec<ExecutedBlock> {
        todo!("implement this")
    }

    /// Prunes block data before the given block hash according to the configured prune
    /// configuration.
    fn prune_before(&mut self, block_num: u64) -> PruneProgress {
        // TODO: doing this properly depends on pruner segment changes
        self.pruner.run(block_num).expect("todo: handle errors")
    }

    /// Removes static file related data from the database, depending on the current block height in
    /// existing static files.
    fn clean_static_file_duplicates(&self) {
        todo!("implement this")
    }
}

impl<DB> Persistence<DB>
where
    DB: Database + 'static,
{
    /// Create a new persistence task, spawning it, and returning a [`PersistenceHandle`].
    fn spawn_new(provider: ProviderFactory<DB>, pruner: Pruner<DB>) -> PersistenceHandle {
        let (tx, rx) = std::sync::mpsc::channel();
        let task = Self::new(provider, rx, pruner);
        std::thread::Builder::new()
            .name("Persistence Task".to_string())
            .spawn(|| task.run())
            .unwrap();

        PersistenceHandle::new(tx)
    }
}

impl<DB> Persistence<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to persistence events and perform the requested
    /// database actions
    fn run(mut self) {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::RemoveBlocksAbove((new_tip_num, sender)) => {
                    let output = self.remove_blocks_above(new_tip_num);

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(output);
                }
                PersistenceAction::SaveBlocks((blocks, sender)) => {
                    if blocks.is_empty() {
                        todo!("return error or something");
                    }
                    let last_block_hash = blocks.last().unwrap().block().hash();
                    self.write(blocks).unwrap();

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(last_block_hash);
                }
                PersistenceAction::PruneBefore((block_num, sender)) => {
                    let res = self.prune_before(block_num);

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(res);
                }
                PersistenceAction::CleanStaticFileDuplicates(sender) => {
                    self.clean_static_file_duplicates();

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(());
                }
            }
        }
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
#[derive(Debug)]
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    SaveBlocks((Vec<ExecutedBlock>, oneshot::Sender<B256>)),

    /// Removes the blocks above the given block number from the database.
    RemoveBlocksAbove((u64, oneshot::Sender<Vec<ExecutedBlock>>)),

    /// Prune associated block data before the given block number, according to already-configured
    /// prune modes.
    PruneBefore((u64, oneshot::Sender<PruneProgress>)),

    /// Trigger a read of static file data, and delete data depending on the highest block in each
    /// static file segment.
    CleanStaticFileDuplicates(oneshot::Sender<()>),
}

/// A handle to the persistence task
#[derive(Debug, Clone)]
pub struct PersistenceHandle {
    /// The channel used to communicate with the persistence task
    sender: Sender<PersistenceAction>,
}

impl PersistenceHandle {
    /// Create a new [`PersistenceHandle`] from a [`Sender<PersistenceAction>`].
    pub const fn new(sender: Sender<PersistenceAction>) -> Self {
        Self { sender }
    }

    /// Sends a specific [`PersistenceAction`] in the contained channel. The caller is responsible
    /// for creating any channels for the given action.
    pub fn send_action(
        &self,
        action: PersistenceAction,
    ) -> Result<(), SendError<PersistenceAction>> {
        self.sender.send(action)
    }

    /// Tells the persistence task to save a certain list of finalized blocks. The blocks are
    /// assumed to be ordered by block number.
    ///
    /// This returns the latest hash that has been saved, allowing removal of that block and any
    /// previous blocks from in-memory data structures.
    pub async fn save_blocks(&self, blocks: Vec<ExecutedBlock>) -> B256 {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::SaveBlocks((blocks, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the persistence task to remove blocks above a certain block number. The removed blocks
    /// are returned by the task.
    pub async fn remove_blocks_above(&self, block_num: u64) -> Vec<ExecutedBlock> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::RemoveBlocksAbove((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the persistence task to remove block data before the given hash, according to the
    /// configured prune config.
    pub async fn prune_before(&self, block_num: u64) -> PruneProgress {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::PruneBefore((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the persistence task to read static file data, and delete data depending on the
    /// highest block in each static file segment.
    pub async fn clean_static_file_duplicates(&self) {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::CleanStaticFileDuplicates(tx))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }
}
