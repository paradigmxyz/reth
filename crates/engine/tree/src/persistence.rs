#![allow(dead_code)]

use crate::{
    database::{DatabaseAction, DatabaseService, DatabaseServiceHandle},
    static_files::{StaticFileAction, StaticFileService, StaticFileServiceHandle},
    tree::ExecutedBlock,
};
use reth_db::Database;
use reth_primitives::{SealedBlock, B256, U256};
use reth_provider::ProviderFactory;
use reth_prune::{Pruner, PrunerOutput};
use std::sync::{
    mpsc::{SendError, Sender},
    Arc,
};
use tokio::sync::oneshot;

/// A signal to the database and static file services that part of the tree state can be persisted.
#[derive(Debug)]
pub enum PersistenceAction {
    /// The given block has been added to the canonical chain, its transactions and headers will be
    /// persisted for durability.
    LogTransactions((Arc<SealedBlock>, u64, U256, oneshot::Sender<()>)),

    /// The section of tree state that should be persisted. These blocks are expected in order of
    /// increasing block number.
    ///
    /// This should just store the execution history-related data. Header, transaction, and
    /// receipt-related data should already be written to static files.
    SaveBlocks((Vec<ExecutedBlock>, oneshot::Sender<B256>)),

    /// Removes block data above the given block number from the database.
    RemoveBlocksAbove((u64, oneshot::Sender<()>)),

    /// Prune associated block data before the given block number, according to already-configured
    /// prune modes.
    PruneBefore((u64, oneshot::Sender<PrunerOutput>)),
}

/// An error type for when there is a [`SendError`] while sending an action to one of the services.
#[derive(Debug)]
pub enum PersistenceSendError {
    /// When there is an error sending to the static file service
    StaticFile(SendError<StaticFileAction>),
    /// When there is an error sending to the database service
    Database(SendError<DatabaseAction>),
}

impl From<SendError<StaticFileAction>> for PersistenceSendError {
    fn from(value: SendError<StaticFileAction>) -> Self {
        Self::StaticFile(value)
    }
}

impl From<SendError<DatabaseAction>> for PersistenceSendError {
    fn from(value: SendError<DatabaseAction>) -> Self {
        Self::Database(value)
    }
}

/// A handle to the database and static file services. This will send commands to the correct
/// service, depending on the command.
///
/// Some commands should be sent to the database service, and others should be sent to the static
/// file service, despite having the same name. This is because some actions require work to be done
/// by both the static file _and_ the database service, and require some coordination.
///
/// This type is what actually coordinates the two services, and should be used by consumers of the
/// persistence related services.
#[derive(Debug, Clone)]
pub struct PersistenceHandle {
    /// The channel used to communicate with the database service
    db_sender: Sender<DatabaseAction>,
    /// The channel used to communicate with the static file service
    static_file_sender: Sender<StaticFileAction>,
}

impl PersistenceHandle {
    /// Create a new [`PersistenceHandle`] from a [`Sender<PersistenceAction>`].
    pub const fn new(
        db_sender: Sender<DatabaseAction>,
        static_file_sender: Sender<StaticFileAction>,
    ) -> Self {
        Self { db_sender, static_file_sender }
    }

    /// Create a new [`PersistenceHandle`], and spawn the database and static file services.
    pub fn spawn_services<DB: Database + 'static>(
        provider_factory: ProviderFactory<DB>,
        pruner: Pruner<DB, ProviderFactory<DB>>,
    ) -> Self {
        // create the initial channels
        let (static_file_service_tx, static_file_service_rx) = std::sync::mpsc::channel();
        let (db_service_tx, db_service_rx) = std::sync::mpsc::channel();

        // construct persistence handle
        let persistence_handle = Self::new(db_service_tx.clone(), static_file_service_tx.clone());

        // construct handles for the services to talk to each other
        let static_file_handle = StaticFileServiceHandle::new(static_file_service_tx);
        let database_handle = DatabaseServiceHandle::new(db_service_tx);

        // spawn the db service
        let db_service = DatabaseService::new(
            provider_factory.clone(),
            db_service_rx,
            static_file_handle,
            pruner,
        );
        std::thread::Builder::new()
            .name("Database Service".to_string())
            .spawn(|| db_service.run())
            .unwrap();

        // spawn the static file service
        let static_file_service =
            StaticFileService::new(provider_factory, static_file_service_rx, database_handle);
        std::thread::Builder::new()
            .name("Static File Service".to_string())
            .spawn(|| static_file_service.run())
            .unwrap();

        persistence_handle
    }

    /// Sends a specific [`PersistenceAction`] in the contained channel. The caller is responsible
    /// for creating any channels for the given action.
    pub fn send_action(&self, action: PersistenceAction) -> Result<(), PersistenceSendError> {
        match action {
            PersistenceAction::LogTransactions(input) => self
                .static_file_sender
                .send(StaticFileAction::LogTransactions(input))
                .map_err(From::from),
            PersistenceAction::SaveBlocks(input) => self
                .static_file_sender
                .send(StaticFileAction::WriteExecutionData(input))
                .map_err(From::from),
            PersistenceAction::RemoveBlocksAbove(input) => {
                self.db_sender.send(DatabaseAction::RemoveBlocksAbove(input)).map_err(From::from)
            }
            PersistenceAction::PruneBefore(input) => {
                self.db_sender.send(DatabaseAction::PruneBefore(input)).map_err(From::from)
            }
        }
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
    use crate::test_utils::{get_executed_block_with_number, get_executed_blocks};
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
        let persistence_handle = default_persistence_handle();

        let blocks = vec![];
        let (tx, rx) = oneshot::channel();

        persistence_handle.save_blocks(blocks, tx);

        let hash = rx.await.unwrap();
        assert_eq!(hash, B256::default());
    }

    #[tokio::test]
    async fn test_save_blocks_single_block() {
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
