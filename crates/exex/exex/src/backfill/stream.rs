use super::job::SingleBlockBackfillJob;
use futures::{
    stream::{FuturesOrdered, Stream},
    StreamExt,
};
use reth_evm::execute::{BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider};
use reth_primitives::{BlockNumber, BlockWithSenders, Receipt};
use reth_provider::{BlockReader, HeaderProvider, StateProviderFactory};
use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
};
use tokio::task::JoinHandle;

type BackfillTasks = FuturesOrdered<
    JoinHandle<Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>>,
>;

/// The default parallelism for active tasks in [`BackFillJobStream`].
const DEFAULT_PARALLELISM: usize = 4;

/// Stream for processing backfill jobs asynchronously.
///
/// This struct manages the execution of [`SingleBlockBackfillJob`] tasks, allowing blocks to be
/// processed asynchronously but in order within a specified range.
#[derive(Debug)]
pub struct BackFillJobStream<E, P> {
    job: SingleBlockBackfillJob<E, P>,
    tasks: BackfillTasks,
    range: RangeInclusive<BlockNumber>,
    parallelism: usize,
}

impl<E, P> BackFillJobStream<E, P>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + 'static,
{
    /// Creates a new [`BackFillJobStream`] with the default parallelism.
    ///
    /// # Parameters
    /// - `job`: The [`SingleBlockBackfillJob`] to be executed asynchronously.
    ///
    /// # Returns
    /// A new instance of [`BackFillJobStream`] with the default parallelism.
    pub fn new(job: SingleBlockBackfillJob<E, P>) -> Self {
        let range = job.range.clone();
        Self { job, tasks: FuturesOrdered::new(), range, parallelism: DEFAULT_PARALLELISM }
    }

    /// Configures the parallelism of the [`BackFillJobStream`] to handle active tasks.
    ///
    /// # Parameters
    /// - `parallelism`: The parallelism to handle active tasks.
    ///
    /// # Returns
    /// The modified instance of [`BackFillJobStream`] with the specified parallelism.
    pub const fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self
    }

    fn spawn_task(
        &self,
        block_number: BlockNumber,
    ) -> JoinHandle<Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>>
    {
        let job = self.job.clone();
        tokio::task::spawn_blocking(move || job.execute_block(block_number))
    }
}

impl<E, P> Stream for BackFillJobStream<E, P>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + 'static + Unpin,
{
    type Item = Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Spawn new tasks only if we are below the parallelism configured.
        while this.tasks.len() < this.parallelism {
            if let Some(block_number) = this.range.next() {
                let task = this.spawn_task(block_number);
                this.tasks.push_back(task);
            } else {
                break;
            }
        }

        match ready!(this.tasks.poll_next_unpin(cx)) {
            Some(res) => Poll::Ready(Some(res.map_err(|e| BlockExecutionError::Other(e.into()))?)),
            None => Poll::Ready(None),
        }
    }
}
