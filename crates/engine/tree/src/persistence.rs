use crate::tree::ExecutedBlock;
use reth_db::database::Database;
use reth_errors::ProviderResult;
use reth_primitives::B256;
use reth_provider::ProviderFactory;
use std::sync::mpsc::Receiver;
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
    fn remove_blocks_above(&mut self, block_number: u64) -> Vec<ExecutedBlock> {
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
        // TODO: err handling?
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
                    self.write(blocks).unwrap();
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
