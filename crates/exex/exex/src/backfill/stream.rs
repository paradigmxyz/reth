use crate::{BackfillJob, SingleBlockBackfillJob};
use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
};

use alloy_primitives::BlockNumber;
use futures::{
    stream::{FuturesOrdered, Stream},
    StreamExt,
};
use reth_evm::execute::{BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider};
use reth_primitives::{BlockWithSenders, Receipt};
use reth_provider::{BlockReader, Chain, HeaderProvider, StateProviderFactory};
use reth_prune_types::PruneModes;
use reth_stages_api::ExecutionStageThresholds;
use reth_tracing::tracing::debug;
use tokio::task::JoinHandle;

use super::job::BackfillJobResult;

/// The default parallelism for active tasks in [`StreamBackfillJob`].
pub(crate) const DEFAULT_PARALLELISM: usize = 4;
/// The default batch size for active tasks in [`StreamBackfillJob`].
const DEFAULT_BATCH_SIZE: usize = 100;

/// Boxed thread-safe iterator that yields [`BackfillJobResult`]s.
type BackfillTaskIterator<T> =
    Box<dyn Iterator<Item = BackfillJobResult<T>> + Send + Sync + 'static>;

/// Backfill task output.
struct BackfillTaskOutput<T> {
    job: BackfillTaskIterator<T>,
    result: Option<BackfillJobResult<T>>,
}

/// Ordered queue of [`JoinHandle`]s that yield [`BackfillTaskOutput`]s.
type BackfillTasks<T> = FuturesOrdered<JoinHandle<BackfillTaskOutput<T>>>;

type SingleBlockStreamItem = (BlockWithSenders, BlockExecutionOutput<Receipt>);
type BatchBlockStreamItem = Chain;

/// Stream for processing backfill jobs asynchronously.
///
/// This struct manages the execution of [`SingleBlockBackfillJob`] tasks, allowing blocks to be
/// processed asynchronously but in order within a specified range.
#[derive(Debug)]
pub struct StreamBackfillJob<E, P, T> {
    executor: E,
    provider: P,
    prune_modes: PruneModes,
    range: RangeInclusive<BlockNumber>,
    tasks: BackfillTasks<T>,
    parallelism: usize,
    batch_size: usize,
    thresholds: ExecutionStageThresholds,
}

impl<E, P, T> StreamBackfillJob<E, P, T>
where
    T: Send + Sync + 'static,
{
    /// Configures the parallelism of the [`StreamBackfillJob`] to handle active tasks.
    pub const fn with_parallelism(mut self, parallelism: usize) -> Self {
        self.parallelism = parallelism;
        self
    }

    /// Configures the batch size for the [`StreamBackfillJob`].
    pub const fn with_batch_size(mut self, batch_size: usize) -> Self {
        self.batch_size = batch_size;
        self
    }

    /// Spawns a new task calling the [`BackfillTaskIterator::next`] method and pushes it to the
    /// [`BackfillTasks`] queue.
    fn push_task(&mut self, mut job: BackfillTaskIterator<T>) {
        self.tasks.push_back(tokio::task::spawn_blocking(move || BackfillTaskOutput {
            result: job.next(),
            job,
        }));
    }

    /// Polls the next task in the [`BackfillTasks`] queue until it returns a non-empty result.
    fn poll_next_task(&mut self, cx: &mut Context<'_>) -> Poll<Option<BackfillJobResult<T>>> {
        while let Some(res) = ready!(self.tasks.poll_next_unpin(cx)) {
            let task_result = res.map_err(BlockExecutionError::other)?;

            if let BackfillTaskOutput { result: Some(job_result), job } = task_result {
                // If the task returned a non-empty result, a new task advancing the job is created
                // and pushed to the front of the queue.
                self.push_task(job);

                return Poll::Ready(Some(job_result))
            };
        }

        Poll::Ready(None)
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, SingleBlockStreamItem>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + Unpin + 'static,
{
    type Item = BackfillJobResult<SingleBlockStreamItem>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Spawn new tasks only if we are below the parallelism configured.
        while this.tasks.len() < this.parallelism {
            // Get the next block number from the range. If it is empty, we are done.
            let Some(block_number) = this.range.next() else {
                debug!(target: "exex::backfill", tasks = %this.tasks.len(), range = ?this.range, "No more single blocks to backfill");
                break;
            };

            // Spawn a new task for that block
            debug!(target: "exex::backfill", tasks = %this.tasks.len(), ?block_number, "Spawning new single block backfill task");
            let job = Box::new(SingleBlockBackfillJob {
                executor: this.executor.clone(),
                provider: this.provider.clone(),
                range: block_number..=block_number,
                stream_parallelism: this.parallelism,
            }) as BackfillTaskIterator<_>;
            this.push_task(job);
        }

        this.poll_next_task(cx)
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, BatchBlockStreamItem>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + Unpin + 'static,
{
    type Item = BackfillJobResult<BatchBlockStreamItem>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Spawn new tasks only if we are below the parallelism configured.
        while this.tasks.len() < this.parallelism {
            // Take the next `batch_size` blocks from the range and calculate the range bounds
            let mut range = this.range.by_ref().take(this.batch_size);
            let start = range.next();
            let range_bounds = start.zip(range.last().or(start));

            // Create the range from the range bounds. If it is empty, we are done.
            let Some(range) = range_bounds.map(|(first, last)| first..=last) else {
                debug!(target: "exex::backfill", tasks = %this.tasks.len(), range = ?this.range, "No more block batches to backfill");
                break;
            };

            // Spawn a new task for that range
            debug!(target: "exex::backfill", tasks = %this.tasks.len(), ?range, "Spawning new block batch backfill task");
            let job = Box::new(BackfillJob {
                executor: this.executor.clone(),
                provider: this.provider.clone(),
                prune_modes: this.prune_modes.clone(),
                thresholds: this.thresholds.clone(),
                range,
                stream_parallelism: this.parallelism,
            }) as BackfillTaskIterator<_>;
            this.push_task(job);
        }

        this.poll_next_task(cx)
    }
}

impl<E, P> From<SingleBlockBackfillJob<E, P>> for StreamBackfillJob<E, P, SingleBlockStreamItem> {
    fn from(job: SingleBlockBackfillJob<E, P>) -> Self {
        Self {
            executor: job.executor,
            provider: job.provider,
            prune_modes: PruneModes::default(),
            range: job.range,
            tasks: FuturesOrdered::new(),
            parallelism: job.stream_parallelism,
            batch_size: 1,
            thresholds: ExecutionStageThresholds { max_blocks: Some(1), ..Default::default() },
        }
    }
}

impl<E, P> From<BackfillJob<E, P>> for StreamBackfillJob<E, P, BatchBlockStreamItem> {
    fn from(job: BackfillJob<E, P>) -> Self {
        let batch_size = job.thresholds.max_blocks.map_or(DEFAULT_BATCH_SIZE, |max| max as usize);
        Self {
            executor: job.executor,
            provider: job.provider,
            prune_modes: job.prune_modes,
            range: job.range,
            tasks: FuturesOrdered::new(),
            parallelism: job.stream_parallelism,
            batch_size,
            thresholds: ExecutionStageThresholds {
                max_blocks: Some(batch_size as u64),
                ..job.thresholds
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        backfill::test_utils::{
            blocks_and_execution_outcome, blocks_and_execution_outputs, chain_spec,
        },
        BackfillJobFactory,
    };
    use futures::StreamExt;
    use reth_blockchain_tree::noop::NoopBlockchainTree;
    use reth_db_common::init::init_genesis;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::public_key_to_address;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    };
    use reth_stages_api::ExecutionStageThresholds;
    use reth_testing_utils::generators;
    use secp256k1::Keypair;

    #[tokio::test]
    async fn test_single_blocks() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = chain_spec(address);

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory)?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        // Create first 2 blocks
        let blocks_and_execution_outcomes =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor.clone(), blockchain_db.clone());
        let mut backfill_stream = factory.backfill(1..=1).into_single_blocks().into_stream();

        // execute first block
        let (block, mut execution_output) = backfill_stream.next().await.unwrap().unwrap();
        execution_output.state.reverts.sort();
        let sealed_block_with_senders = blocks_and_execution_outcomes[0].0.clone();
        let expected_block = sealed_block_with_senders.unseal();
        let expected_output = &blocks_and_execution_outcomes[0].1;
        assert_eq!(block, expected_block);
        assert_eq!(&execution_output, expected_output);

        // expect no more blocks
        assert!(backfill_stream.next().await.is_none());

        Ok(())
    }

    #[tokio::test]
    async fn test_batch() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = chain_spec(address);

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory)?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        // Create first 2 blocks
        let (blocks, execution_outcome) =
            blocks_and_execution_outcome(provider_factory, chain_spec, key_pair)?;

        // Backfill the same range
        let factory =
            BackfillJobFactory::new(executor.clone(), blockchain_db.clone()).with_thresholds(
                ExecutionStageThresholds { max_blocks: Some(2), ..Default::default() },
            );
        let mut backfill_stream = factory.backfill(1..=2).into_stream();
        let mut chain = backfill_stream.next().await.unwrap().unwrap();
        chain.execution_outcome_mut().state_mut().reverts.sort();

        assert!(chain.blocks_iter().eq(&blocks));
        assert_eq!(chain.execution_outcome(), &execution_outcome);

        // expect no more blocks
        assert!(backfill_stream.next().await.is_none());

        Ok(())
    }
}
