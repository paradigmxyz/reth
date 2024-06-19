use crate::tree::ExecutedBlock;
use futures::{ready, FutureExt};
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use std::{
    pin::Pin,
    task::{Context, Poll},
};
use tokio::{
    sync::{mpsc::Receiver, oneshot},
    task::{spawn_blocking, JoinHandle},
};

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

impl<Writer> Persistence<Writer> {
    // TODO: initialization
    /// Writes the cloned tree state to the database
    fn write(&mut self, blocks: Vec<ExecutedBlock>) {
        todo!("implement this")
    }

    /// Removes the blocks above the give block number from the database, returning them.
    fn remove_blocks_above(&mut self, block_number: u64) -> Vec<ExecutedBlock> {
        todo!("implement this")
    }
}

impl<Writer> Persistence<Writer>
where
    Writer: Unpin,
{
    /// This is the main loop, that will listen to persistence events and perform the requested
    /// database actions
    async fn run(&mut self) {
        // TODO: sync or async receiver?
        while let Some(action) = self.incoming.recv().await {
            match action {
                PersistenceAction::RemoveBlocksAbove((new_tip_num, sender)) => {
                    // spawn blocking so we can poll the thread later
                    let output = this.remove_blocks_above(new_tip_num);
                    sender.send(output).unwrap();
                }
                PersistenceAction::SaveFinalizedBlocks((blocks, sender)) => {
                    if blocks.is_empty() {
                        todo!("return error or something");
                    }
                    let last_block_hash = blocks.last().unwrap().block().hash();
                    this.write(blocks);
                }
            }
        }
    }
}

/// A signal to the persistence task that part of the tree state can be persisted.
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    SaveFinalizedBlocks((Vec<ExecutedBlock>, oneshot::Sender<B256>)),

    /// Removes the blocks above the given block number from the database.
    RemoveBlocksAbove((u64, oneshot::Sender<Vec<ExecutedBlock>>)),
}
