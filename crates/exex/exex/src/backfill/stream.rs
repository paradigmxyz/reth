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

/// The default parallelism for active tasks in [`StreamBackfillJob`].
pub(crate) const DEFAULT_PARALLELISM: usize = 4;
/// The default batch size for active tasks in [`StreamBackfillJob`].
const DEFAULT_BATCH_SIZE: usize = 100;

type BackfillTasks<T> = FuturesOrdered<JoinHandle<Result<T, BlockExecutionError>>>;

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

impl<E, P, T> StreamBackfillJob<E, P, T> {
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

    fn poll_next_task(
        &mut self,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<T, BlockExecutionError>>> {
        match ready!(self.tasks.poll_next_unpin(cx)) {
            Some(res) => Poll::Ready(Some(res.map_err(BlockExecutionError::other)?)),
            None => Poll::Ready(None),
        }
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, SingleBlockStreamItem>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + Unpin + 'static,
{
    type Item = Result<SingleBlockStreamItem, BlockExecutionError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Spawn new tasks only if we are below the parallelism configured.
        while this.tasks.len() < this.parallelism {
            // If we have a block number, then we can spawn a new task for that block
            if let Some(block_number) = this.range.next() {
                let mut job = SingleBlockBackfillJob {
                    executor: this.executor.clone(),
                    provider: this.provider.clone(),
                    range: block_number..=block_number,
                    stream_parallelism: this.parallelism,
                };
                let task =
                    tokio::task::spawn_blocking(move || job.next().expect("non-empty range"));
                this.tasks.push_back(task);
            } else {
                break;
            }
        }

        this.poll_next_task(cx)
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, BatchBlockStreamItem>
where
    E: BlockExecutorProvider + Clone + Send + 'static,
    P: HeaderProvider + BlockReader + StateProviderFactory + Clone + Send + Unpin + 'static,
{
    type Item = Result<BatchBlockStreamItem, BlockExecutionError>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        // Spawn new tasks only if we are below the parallelism configured.
        while this.tasks.len() < this.parallelism {
            // Take the next `batch_size` blocks from the range and calculate the range bounds
            let mut range = this.range.by_ref().take(this.batch_size);
            let start = range.next();
            let range_bounds = start.zip(range.last().or(start));

            // If we have range bounds, then we can spawn a new task for that range
            if let Some((first, last)) = range_bounds {
                let range = first..=last;
                debug!(target: "exex::backfill", tasks = %this.tasks.len(), ?range, "Spawning new backfill task");
                let mut job = BackfillJob {
                    executor: this.executor.clone(),
                    provider: this.provider.clone(),
                    prune_modes: this.prune_modes.clone(),
                    thresholds: this.thresholds.clone(),
                    range,
                    stream_parallelism: this.parallelism,
                };
                let task =
                    tokio::task::spawn_blocking(move || job.next().expect("non-empty range"));
                this.tasks.push_back(task);
            } else {
                break;
            }
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
