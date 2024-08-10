#![allow(dead_code)]

use reth_chain_state::ExecutedBlock;
use reth_db::Database;
use reth_errors::ProviderError;
use reth_primitives::B256;
use reth_provider::{writer::UnifiedStorageWriter, ProviderFactory, StaticFileProviderFactory};
use reth_prune::{Pruner, PrunerError, PrunerOutput};
use std::sync::mpsc::{Receiver, SendError, Sender};
use thiserror::Error;
use tokio::sync::oneshot;
use tracing::{debug, error};

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

    /// Prunes block data before the given block hash according to the configured prune
    /// configuration.
    fn prune_before(&mut self, block_num: u64) -> Result<PrunerOutput, PrunerError> {
        debug!(target: "tree::persistence", ?block_num, "Running pruner");
        // TODO: doing this properly depends on pruner segment changes
        self.pruner.run(block_num)
    }
}

impl<DB> PersistenceService<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to database events and perform the requested
    /// database actions
    pub fn run(mut self) -> Result<(), PersistenceError> {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                PersistenceAction::RemoveBlocksAbove(new_tip_num, sender) => {
                    let provider_rw = self.provider.provider_rw()?;
                    let sf_provider = self.provider.static_file_provider();

                    UnifiedStorageWriter::from(&provider_rw, &sf_provider)
                        .remove_blocks_above(new_tip_num)?;
                    UnifiedStorageWriter::commit_unwind(provider_rw, sf_provider)?;

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(());
                }
                PersistenceAction::SaveBlocks(blocks, sender) => {
                    let Some(last_block) = blocks.last() else {
                        let _ = sender.send(None);
                        continue
                    };

                    let last_block_hash = last_block.block().hash();

                    let provider_rw = self.provider.provider_rw()?;
                    let static_file_provider = self.provider.static_file_provider();

                    UnifiedStorageWriter::from(&provider_rw, &static_file_provider)
                        .save_blocks(&blocks)?;
                    UnifiedStorageWriter::commit(provider_rw, static_file_provider)?;

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(Some(last_block_hash));
                }
                PersistenceAction::PruneBefore(block_num, sender) => {
                    let res = self.prune_before(block_num)?;

                    // we ignore the error because the caller may or may not care about the result
                    let _ = sender.send(res);
                }
            }
        }
        Ok(())
    }
}

/// One of the errors that can happen when using the persistence service.
#[derive(Debug, Error)]
pub enum PersistenceError {
    /// A pruner error
    #[error(transparent)]
    PrunerError(#[from] PrunerError),

    /// A provider error
    #[error(transparent)]
    ProviderError(#[from] ProviderError),
}

/// A signal to the persistence service that part of the tree state can be persisted.
#[derive(Debug)]
pub enum PersistenceAction {
    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    ///
    /// First, header, transaction, and receipt-related data should be written to static files.
    /// Then the execution history-related data will be written to the database.
    SaveBlocks(Vec<ExecutedBlock>, oneshot::Sender<Option<B256>>),

    /// Removes block data above the given block number from the database.
    ///
    /// This will first update checkpoints from the database, then remove actual block data from
    /// static files.
    RemoveBlocksAbove(u64, oneshot::Sender<()>),

    /// Prune associated block data before the given block number, according to already-configured
    /// prune modes.
    PruneBefore(u64, oneshot::Sender<PrunerOutput>),
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
    pub fn spawn_service<DB: Database + 'static>(
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
            .spawn(|| {
                if let Err(err) = db_service.run() {
                    error!(target: "engine::persistence", ?err, "Persistence service failed");
                }
            })
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
    ///
    /// If there are no blocks to persist, then `None` is sent in the sender.
    pub fn save_blocks(
        &self,
        blocks: Vec<ExecutedBlock>,
        tx: oneshot::Sender<Option<B256>>,
    ) -> Result<(), SendError<PersistenceAction>> {
        self.send_action(PersistenceAction::SaveBlocks(blocks, tx))
    }

    /// Tells the persistence service to remove blocks above a certain block number. The removed
    /// blocks are returned by the service.
    ///
    /// When the operation completes, `()` is returned in the receiver end of the sender argument.
    pub fn remove_blocks_above(
        &self,
        block_num: u64,
        tx: oneshot::Sender<()>,
    ) -> Result<(), SendError<PersistenceAction>> {
        self.send_action(PersistenceAction::RemoveBlocksAbove(block_num, tx))
    }

    /// Tells the persistence service to remove block data before the given hash, according to the
    /// configured prune config.
    ///
    /// The resulting [`PrunerOutput`] is returned in the receiver end of the sender argument.
    pub fn prune_before(
        &self,
        block_num: u64,
        tx: oneshot::Sender<PrunerOutput>,
    ) -> Result<(), SendError<PersistenceAction>> {
        self.send_action(PersistenceAction::PruneBefore(block_num, tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use reth_chain_state::test_utils::TestBlockBuilder;
    use reth_exex_types::FinishedExExHeight;
    use reth_primitives::B256;
    use reth_provider::{test_utils::create_test_provider_factory, ProviderFactory};
    use reth_prune::Pruner;

    fn default_persistence_handle() -> PersistenceHandle {
        let provider = create_test_provider_factory();

        let (_finished_exex_height_tx, finished_exex_height_rx) =
            tokio::sync::watch::channel(FinishedExExHeight::NoExExs);

        let pruner = Pruner::<_, ProviderFactory<_>>::new(
            provider.clone(),
            vec![],
            5,
            0,
            None,
            finished_exex_height_rx,
        );

        PersistenceHandle::spawn_service(provider, pruner)
    }

    #[tokio::test]
    async fn test_save_blocks_empty() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let blocks = vec![];
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx).unwrap();

        let hash = rx.await.unwrap();
        assert_eq!(hash, None);
    }

    #[tokio::test]
    async fn test_save_blocks_single_block() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();
        let block_number = 0;
        let mut test_block_builder = TestBlockBuilder::default();
        let executed =
            test_block_builder.get_executed_block_with_number(block_number, B256::random());
        let block_hash = executed.block().hash();

        let blocks = vec![executed];
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx).unwrap();

        let actual_hash = tokio::time::timeout(std::time::Duration::from_secs(10), rx)
            .await
            .expect("test timed out")
            .expect("channel closed unexpectedly")
            .expect("no hash returned");

        assert_eq!(block_hash, actual_hash);
    }

    #[tokio::test]
    async fn test_save_blocks_multiple_blocks() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let mut test_block_builder = TestBlockBuilder::default();
        let blocks = test_block_builder.get_executed_blocks(0..5).collect::<Vec<_>>();
        let last_hash = blocks.last().unwrap().block().hash();
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx).unwrap();

        let actual_hash = rx.await.unwrap().unwrap();
        assert_eq!(last_hash, actual_hash);
    }

    #[tokio::test]
    async fn test_save_blocks_multiple_calls() {
        reth_tracing::init_test_tracing();
        let persistence_handle = default_persistence_handle();

        let ranges = [0..1, 1..2, 2..4, 4..5];
        let mut test_block_builder = TestBlockBuilder::default();
        for range in ranges {
            let blocks = test_block_builder.get_executed_blocks(range).collect::<Vec<_>>();
            let last_hash = blocks.last().unwrap().block().hash();
            let (tx, rx) = oneshot::channel();

            persistence_handle.save_blocks(blocks, tx).unwrap();

            let actual_hash = rx.await.unwrap().unwrap();
            assert_eq!(last_hash, actual_hash);
        }
    }
}
