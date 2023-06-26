//! A validation service for transactions.

use crate::validate::TransactionValidatorError;
use futures_util::{lock::Mutex, StreamExt};
use std::{future::Future, pin::Pin, sync::Arc};
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;

/// A service that performs validation jobs.
#[derive(Clone)]
pub struct ValidationTask {
    #[allow(clippy::type_complexity)]
    validation_jobs: Arc<Mutex<ReceiverStream<Pin<Box<dyn Future<Output = ()> + Send>>>>>,
}

impl ValidationTask {
    /// Creates a new clonable task pair
    pub fn new() -> (ValidationJobSender, Self) {
        let (tx, rx) = mpsc::channel(1);
        (ValidationJobSender { tx }, Self::with_receiver(rx))
    }

    /// Creates a new task with the given receiver.
    pub fn with_receiver(jobs: mpsc::Receiver<Pin<Box<dyn Future<Output = ()> + Send>>>) -> Self {
        ValidationTask { validation_jobs: Arc::new(Mutex::new(ReceiverStream::new(jobs))) }
    }

    /// Executes all new validation jobs that come in.
    ///
    /// This will run as long as the channel is alive and is expected to be spawned as a task.
    pub async fn run(self) {
        loop {
            let task = self.validation_jobs.lock().await.next().await;
            match task {
                None => return,
                Some(task) => task.await,
            }
        }
    }
}

impl std::fmt::Debug for ValidationTask {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ValidationTask").field("validation_jobs", &"...").finish()
    }
}

/// A sender new type for sending validation jobs to [ValidationTask].
#[derive(Debug)]
pub struct ValidationJobSender {
    tx: mpsc::Sender<Pin<Box<dyn Future<Output = ()> + Send>>>,
}

impl ValidationJobSender {
    /// Sends the given job to the validation task.
    pub async fn send(
        &self,
        job: Pin<Box<dyn Future<Output = ()> + Send>>,
    ) -> Result<(), TransactionValidatorError> {
        self.tx.send(job).await.map_err(|_| TransactionValidatorError::ValidationServiceUnreachable)
    }
}
