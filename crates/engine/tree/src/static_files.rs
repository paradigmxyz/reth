#![allow(dead_code)]

use reth_db::database::Database;
use reth_primitives::BlockNumber;
use reth_provider::ProviderFactory;
use reth_static_file::{HighestStaticFiles, StaticFileProducerInner};
use std::sync::mpsc::{Receiver, Sender};
use tokio::sync::oneshot;

/// Writes finalized blocks to reth's static files.
///
/// This is meant to be a spawned task that listens for various incoming finalization operations,
/// and writing to or producing new static files.
///
/// This should be spawned in its own thread with [`std::thread::spawn`], since this performs
/// blocking file operations in an endless loop.
#[derive(Debug)]
pub struct StaticFileTask<DB> {
    /// The db / static file provider to use
    provider: ProviderFactory<DB>,
    /// The static file producer to use
    static_file_producer: StaticFileProducerInner<DB>,
    /// Incoming requests to write static files
    incoming: Receiver<StaticFileAction>,
}

impl<DB> StaticFileTask<DB>
where
    DB: Database + 'static,
{
    /// Create a new static file task, spawning it, and returning a [`StaticFileTaskHandle`].
    fn spawn_new(provider: ProviderFactory<DB>) -> StaticFileTaskHandle {
        todo!("implement initialization first");
        // let (tx, rx) = std::sync::mpsc::channel();
        // let task = Self::new(provider, rx);
        // std::thread::Builder::new()
        //     .name("StaticFile Task".to_string())
        //     .spawn(|| task.run())
        //     .unwrap();

        // StaticFileTaskHandle::new(tx)
    }
}

impl<DB> StaticFileTask<DB>
where
    DB: Database,
{
    /// This is the main loop, that will listen to finalization actions, and write DB data to static
    /// files.
    fn run(self) {
        // If the receiver errors then senders have disconnected, so the loop should then end.
        while let Ok(action) = self.incoming.recv() {
            match action {
                StaticFileAction::FinalizedBlock((new_finalized_num, sender)) => {
                    let finalized_block_nums = HighestStaticFiles {
                        headers: Some(new_finalized_num),
                        transactions: Some(new_finalized_num),
                        receipts: Some(new_finalized_num),
                    };
                    let targets = self
                        .static_file_producer
                        .get_static_file_targets(finalized_block_nums)
                        .unwrap();
                    self.static_file_producer.run(targets).unwrap();

                    // The caller may not care about the result so we ignore the error
                    let _ = sender.send(());
                }
            }
        }
    }
}

/// A signal to the static file task that some data should be copied from the DB to static files.
#[derive(Debug)]
pub enum StaticFileAction {
    /// The given block number is the newest finalized block, and data from the DB can be copied
    /// over to static files.
    FinalizedBlock((BlockNumber, oneshot::Sender<()>)),
}

/// A handle to the static file task
#[derive(Debug, Clone)]
pub struct StaticFileTaskHandle {
    /// The channel used to communicate with the static file task
    sender: Sender<StaticFileAction>,
}

impl StaticFileTaskHandle {
    /// Create a new [`StaticFileTaskHandle`] from a [`Sender<StaticFileTaskAction>`].
    pub const fn new(sender: Sender<StaticFileAction>) -> Self {
        Self { sender }
    }

    /// Writes to static files based on what the new finalized block is.
    pub async fn finalized_block(&self, block_num: BlockNumber) {
        let (tx, rx) = oneshot::channel();
        self.sender
            .send(StaticFileAction::FinalizedBlock((block_num, tx)))
            .expect("should be able to send");
        rx.await.expect("todo: err handling")
    }
}
