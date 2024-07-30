use crate::SingleBlockBackfillJob;
use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
};

use futures::{
    stream::{FuturesOrdered, Stream},
    StreamExt,
};
use reth_evm::execute::{BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider};
use reth_primitives::{BlockNumber, BlockWithSenders, Receipt};
use reth_provider::{BlockReader, HeaderProvider, StateProviderFactory};
use tokio::task::JoinHandle;

type BackfillTasks = FuturesOrdered<
    JoinHandle<Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>>,
>;

/// The default parallelism for active tasks in [`BackFillJobStream`].
pub(crate) const DEFAULT_PARALLELISM: usize = 4;

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

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        backfill::test_utils::{blocks_and_execution_outputs, chain_spec},
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
    use reth_testing_utils::generators;
    use secp256k1::Keypair;

    #[tokio::test]
    async fn test_async_backfill() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = Keypair::new_global(&mut generators::rng());
        let address = public_key_to_address(key_pair.public_key());

        let chain_spec = chain_spec(address);

        let executor = EthExecutorProvider::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(provider_factory.clone())?;
        let blockchain_db = BlockchainProvider::new(
            provider_factory.clone(),
            Arc::new(NoopBlockchainTree::default()),
        )?;

        // Create first 2 blocks
        let blocks_and_execution_outcomes =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor.clone(), blockchain_db.clone());
        let mut backfill_stream = factory.backfill(1..=1).into_stream();

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
}
