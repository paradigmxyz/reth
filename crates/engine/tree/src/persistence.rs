use crate::tree::ExecutedBlock;
use reth_db::database::Database;
use reth_errors::ProviderResult;
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use std::sync::mpsc::{Receiver, Sender};
use tokio::sync::oneshot;

/// Writes parts of reth's in memory tree state to the database.
///
/// It's expected that this will be spawned in its own thread with [`std::thread::spawn`], since
/// this performs blocking database operations.
pub struct Persistence<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    /// Incoming requests to persist stuff
    incoming: Receiver<PersistenceAction>,
}

impl<DB: Database> Persistence<DB> {
    // TODO: initialization
    /// Writes the cloned tree state to the database
    fn write(&mut self, blocks: Vec<ExecutedBlock>) -> ProviderResult<()> {
        let mut rw = self.provider.provider_rw()?;
        todo!("implement this")
    }

    /// Removes the blocks above the give block number from the database, returning them.
    fn remove_blocks_above(&mut self, block_number: u64) -> ProviderResult<Vec<ExecutedBlock>> {
        todo!("implement this")
    }
}

impl<DB> Persistence<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to persistence events and perform the requested
    /// database actions
    fn run(&mut self) {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::RemoveBlocksAbove((new_tip_num, sender)) => {
                    // spawn blocking so we can poll the thread later
                    let output = self.remove_blocks_above(new_tip_num);
                    sender.send(output).unwrap();
                }
                PersistenceAction::SaveFinalizedBlocks((blocks, sender)) => {
                    if blocks.is_empty() {
                        todo!("return error or something");
                    }
                    let last_block_hash = blocks.last().unwrap().block().hash();
                    let res = self.write(blocks).map(|_| last_block_hash);
                    sender.send(res).unwrap();
                }
            }
        }
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    SaveFinalizedBlocks((Vec<ExecutedBlock>, oneshot::Sender<ProviderResult<B256>>)),

    /// Removes the blocks above the given block number from the database.
    RemoveBlocksAbove((u64, oneshot::Sender<ProviderResult<Vec<ExecutedBlock>>>)),
}

/// A handle to the persistence task
#[derive(Debug, Clone)]
pub struct PersistenceHandle {
    /// The channel used to communicate with the persistence task
    sender: Sender<PersistenceAction>,
}

impl PersistenceHandle {
    /// Tells the persistence task to save a certain list of finalized blocks. The blocks are
    /// assumed to be ordered by block number.
    ///
    /// This returns the latest hash that has been saved, allowing removal of that block and any
    /// previous blocks from in-memory data structures.
    pub async fn save_blocks(&self, blocks: Vec<ExecutedBlock>) -> ProviderResult<B256> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::SaveFinalizedBlocks((blocks, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Tells the persistence task to remove blocks above a certain block number. The removed blocks
    /// are returned by the task.
    pub async fn remove_blocks_above(&self, block_num: u64) -> ProviderResult<Vec<ExecutedBlock>> {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(PersistenceAction::RemoveBlocksAbove((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }

    /// Sends a [`PersistenceAction`] to the task directly
    pub fn send_action(&self, action: PersistenceAction) {
        self.sender.send(action).expect("should be able to send");
    }
}
