use super::job::BackfillJobResult;
use crate::{BackfillJob, SingleBlockBackfillJob};
use alloy_primitives::BlockNumber;
use futures::{
    future::{BoxFuture, Either},
    stream::{FuturesOrdered, Stream},
    StreamExt,
};
use reth_evm::{
    execute::{BlockExecutionError, BlockExecutionOutput},
    ConfigureEvm,
};
use reth_node_api::NodePrimitives;
use reth_primitives_traits::RecoveredBlock;
use reth_provider::{BlockReader, Chain, ProviderError, StateProviderFactory};
use reth_prune_types::PruneModes;
use reth_stages_api::ExecutionStageThresholds;
use std::{
    ops::RangeInclusive,
    pin::Pin,
    task::{ready, Context, Poll},
    time::Duration,
};
use tokio::task::{JoinError, JoinHandle};
use tracing::debug;

/// The default parallelism for active tasks in [`StreamBackfillJob`].
pub(crate) const DEFAULT_PARALLELISM: usize = 4;
/// The default batch size for active tasks in [`StreamBackfillJob`].
const DEFAULT_BATCH_SIZE: usize = 100;
/// Delay before retrying a batch whose historical state is not yet available.
const BACKFILL_RETRY_DELAY: Duration = Duration::from_secs(1);
/// Limit the extra time that backfill consumers can spend buffering live notifications.
const MAX_BACKFILL_RETRIES: u16 = 300;

/// Boxed thread-safe iterator that yields [`BackfillJobResult`]s.
type BackfillTaskIterator<T> =
    Box<dyn Iterator<Item = BackfillJobResult<T>> + Send + Sync + 'static>;

/// Backfill task output.
struct BackfillTaskOutput<T> {
    job: BackfillTaskIterator<T>,
    result: Option<BackfillJobResult<T>>,
}

/// A running blocking task, or a retry waiting on the stream's async timer.
type BackfillTask<T> = Either<
    JoinHandle<BackfillTaskOutput<T>>,
    BoxFuture<'static, Result<BackfillTaskOutput<T>, JoinError>>,
>;

pub(super) type SingleBlockStreamItem<N> = (
    RecoveredBlock<<N as NodePrimitives>::Block>,
    BlockExecutionOutput<<N as NodePrimitives>::Receipt>,
);
type BatchBlockStreamItem<N> = Chain<N>;

/// Stream for processing backfill jobs asynchronously.
///
/// This struct manages the execution of [`SingleBlockBackfillJob`] tasks, allowing blocks to be
/// processed asynchronously but in order within a specified range. Batched backfill retries
/// temporary historical state errors during sync at one-second intervals, up to five minutes of
/// retry delays before yielding the error.
#[derive(Debug)]
pub struct StreamBackfillJob<E, P, T> {
    evm_config: E,
    provider: P,
    prune_modes: PruneModes,
    range: RangeInclusive<BlockNumber>,
    tasks: FuturesOrdered<BackfillTask<T>>,
    retries: u16,
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

    fn backfill_task(mut job: BackfillTaskIterator<T>, retry: bool) -> BackfillTask<T> {
        let next = move || BackfillTaskOutput { result: job.next(), job };
        if retry {
            Either::Right(Box::pin(async move {
                tokio::time::sleep(BACKFILL_RETRY_DELAY).await;
                tokio::task::spawn_blocking(next).await
            }))
        } else {
            Either::Left(tokio::task::spawn_blocking(next))
        }
    }

    /// Polls tasks in order, retrying temporary state errors within a per-item budget.
    fn poll_next_task(
        &mut self,
        cx: &mut Context<'_>,
        max_retries: u16,
    ) -> Poll<Option<BackfillJobResult<T>>> {
        while let Some(res) = ready!(self.tasks.poll_next_unpin(cx)) {
            let task_result = res.map_err(BlockExecutionError::other)?;

            if let BackfillTaskOutput { result: Some(job_result), job } = task_result {
                // Batch iterators retain their range if the historical state read fails.
                let retry = max_retries != 0 &&
                    matches!(
                        job_result
                            .as_ref()
                            .err()
                            .and_then(BlockExecutionError::as_internal)
                            .and_then(|error| error.downcast_other::<ProviderError>()),
                        Some(ProviderError::StateUnavailableDuringSync)
                    );
                // Keep the remaining range at the front so later results cannot overtake it.
                self.tasks.push_front(Self::backfill_task(job, retry));

                if retry && self.retries < max_retries {
                    self.retries += 1;
                } else {
                    self.retries = 0;
                    return Poll::Ready(Some(job_result))
                }
            };
        }

        Poll::Ready(None)
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, SingleBlockStreamItem<E::Primitives>>
where
    E: ConfigureEvm<Primitives: NodePrimitives<Block = P::Block>> + 'static,
    P: BlockReader + StateProviderFactory + Clone + Unpin + 'static,
{
    type Item = BackfillJobResult<SingleBlockStreamItem<E::Primitives>>;

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
                evm_config: this.evm_config.clone(),
                provider: this.provider.clone(),
                range: block_number..=block_number,
                stream_parallelism: this.parallelism,
            }) as BackfillTaskIterator<_>;
            this.tasks.push_back(Self::backfill_task(job, false));
        }

        this.poll_next_task(cx, 0)
    }
}

impl<E, P> Stream for StreamBackfillJob<E, P, BatchBlockStreamItem<E::Primitives>>
where
    E: ConfigureEvm<Primitives: NodePrimitives<Block = P::Block>> + 'static,
    P: BlockReader + StateProviderFactory + Clone + Unpin + 'static,
{
    type Item = BackfillJobResult<BatchBlockStreamItem<E::Primitives>>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let this = self.get_mut();

        loop {
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
                    evm_config: this.evm_config.clone(),
                    provider: this.provider.clone(),
                    prune_modes: this.prune_modes.clone(),
                    thresholds: this.thresholds.clone(),
                    range,
                    stream_parallelism: this.parallelism,
                }) as BackfillTaskIterator<_>;
                this.tasks.push_back(Self::backfill_task(job, false));
            }

            let res = ready!(this.poll_next_task(cx, MAX_BACKFILL_RETRIES));

            if res.is_some() {
                return Poll::Ready(res);
            }

            if this.range.is_empty() {
                // only terminate the stream if there are no more blocks to process
                return Poll::Ready(None);
            }
        }
    }
}

impl<E, P> From<SingleBlockBackfillJob<E, P>>
    for StreamBackfillJob<E, P, SingleBlockStreamItem<E::Primitives>>
where
    E: ConfigureEvm,
{
    fn from(job: SingleBlockBackfillJob<E, P>) -> Self {
        Self {
            evm_config: job.evm_config,
            provider: job.provider,
            prune_modes: PruneModes::default(),
            range: job.range,
            tasks: FuturesOrdered::new(),
            retries: 0,
            parallelism: job.stream_parallelism,
            batch_size: 1,
            thresholds: ExecutionStageThresholds { max_blocks: Some(1), ..Default::default() },
        }
    }
}

impl<E, P> From<BackfillJob<E, P>> for StreamBackfillJob<E, P, BatchBlockStreamItem<E::Primitives>>
where
    E: ConfigureEvm,
{
    fn from(job: BackfillJob<E, P>) -> Self {
        let batch_size = job.thresholds.max_blocks.map_or(DEFAULT_BATCH_SIZE, |max| max as usize);
        Self {
            evm_config: job.evm_config,
            provider: job.provider,
            prune_modes: job.prune_modes,
            range: job.range,
            tasks: FuturesOrdered::new(),
            retries: 0,
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
    use super::*;
    use crate::{
        backfill::test_utils::{
            blocks_and_execution_outcome, blocks_and_execution_outputs, chain_spec,
            execute_block_and_commit_to_database,
        },
        BackfillJobFactory,
    };
    use alloy_consensus::{constants::ETH_TO_WEI, Header, TxEip2930};
    use alloy_primitives::{b256, Address, TxKind, U256};
    use eyre::Result;
    use futures::StreamExt;
    use reth_chainspec::{ChainSpec, EthereumHardfork, MIN_TRANSACTION_GAS};
    use reth_db_common::init::init_genesis;
    use reth_ethereum_primitives::{Block, BlockBody, Transaction};
    use reth_evm_ethereum::EthEvmConfig;
    use reth_primitives_traits::{Block as _, NodePrimitives};
    use reth_provider::{
        providers::{BlockchainProvider, ProviderNodeTypes},
        test_utils::create_test_provider_factory_with_chain_spec,
        ProviderFactory,
    };
    use reth_stages_api::ExecutionStageThresholds;
    use reth_testing_utils::{generators, generators::sign_tx_with_key_pair};
    use secp256k1::Keypair;
    use std::{future, iter, sync::Arc};

    #[tokio::test]
    async fn test_single_blocks() -> eyre::Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = generators::generate_key(&mut generators::rng());
        let address =
            Address::from_raw_public_key(&key_pair.public_key().serialize_uncompressed()[1..]);

        let chain_spec = chain_spec(address);

        let executor = EthEvmConfig::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory)?;
        let blockchain_db = BlockchainProvider::new(provider_factory.clone())?;

        // Create first 2 blocks
        let blocks_and_execution_outcomes =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor.clone(), blockchain_db.clone());
        let mut backfill_stream = factory.backfill(1..=1).into_single_blocks().into_stream();

        // execute first block
        let (block, execution_output) = backfill_stream.next().await.unwrap().unwrap();
        let expected_block = blocks_and_execution_outcomes[0].0.clone();
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
        let key_pair = generators::generate_key(&mut generators::rng());
        let address =
            Address::from_raw_public_key(&key_pair.public_key().serialize_uncompressed()[1..]);

        let chain_spec = chain_spec(address);

        let executor = EthEvmConfig::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory)?;
        let blockchain_db = BlockchainProvider::new(provider_factory.clone())?;

        // Create first 2 blocks
        let (blocks, execution_outcome) =
            blocks_and_execution_outcome(provider_factory, chain_spec, key_pair)?;

        // Backfill the same range
        let factory = BackfillJobFactory::new(executor.clone(), blockchain_db.clone())
            .with_thresholds(ExecutionStageThresholds { max_blocks: Some(2), ..Default::default() })
            .with_stream_parallelism(1);
        let mut backfill_stream = factory.backfill(1..=2).into_stream();
        let chain = backfill_stream.next().await.unwrap().unwrap();

        assert!(chain.blocks_iter().map(|block| block.as_ref()).eq(&blocks));
        assert_eq!(chain.execution_outcome(), &execution_outcome);

        // expect no more blocks
        assert!(backfill_stream.next().await.is_none());

        Ok(())
    }

    #[tokio::test(start_paused = true)]
    async fn backfill_retries_temporary_state_errors_in_order() -> eyre::Result<()> {
        let mut backfill = StreamBackfillJob::<(), (), u64> {
            evm_config: (),
            provider: (),
            prune_modes: PruneModes::default(),
            range: 1..=2,
            tasks: FuturesOrdered::new(),
            retries: 0,
            parallelism: 2,
            batch_size: 1,
            thresholds: ExecutionStageThresholds::default(),
        };
        let temporary_error =
            || Err(BlockExecutionError::other(ProviderError::StateUnavailableDuringSync));
        backfill.tasks.push_back(Either::Right(Box::pin(future::ready(Ok(BackfillTaskOutput {
            result: Some(temporary_error()),
            job: Box::new(iter::once(Ok(1))),
        })))));
        backfill.tasks.push_back(Either::Right(Box::pin(future::ready(Ok(BackfillTaskOutput {
            result: Some(Ok(2)),
            job: Box::new(
                iter::once(Err(BlockExecutionError::other(ProviderError::HeaderNotFound(
                    3.into(),
                ))))
                .chain(iter::repeat_with(temporary_error).take(2)),
            ),
        })))));

        let mut stream = futures::stream::poll_fn(|cx| backfill.poll_next_task(cx, 1));
        assert!(futures::poll!(stream.next()).is_pending());
        tokio::time::advance(BACKFILL_RETRY_DELAY).await;
        assert_eq!(stream.next().await.transpose()?, Some(1));
        assert_eq!(stream.next().await.transpose()?, Some(2));

        let error = stream.next().await.unwrap().unwrap_err();
        assert!(matches!(
            error.as_internal().and_then(|error| error.downcast_other::<ProviderError>()),
            Some(ProviderError::HeaderNotFound(block)) if *block == 3.into()
        ));
        let retry_start = tokio::time::Instant::now();
        let error = stream.next().await.unwrap().unwrap_err();
        assert!(matches!(
            error.as_internal().and_then(|error| error.downcast_other::<ProviderError>()),
            Some(ProviderError::StateUnavailableDuringSync)
        ));
        assert!(retry_start.elapsed() >= BACKFILL_RETRY_DELAY);

        Ok(())
    }

    fn create_blocks(
        chain_spec: &Arc<ChainSpec>,
        key_pair: Keypair,
        n: u64,
    ) -> Result<Vec<RecoveredBlock<reth_ethereum_primitives::Block>>> {
        let mut blocks = Vec::with_capacity(n as usize);
        let mut parent_hash = chain_spec.genesis_hash();

        for (i, nonce) in (1..=n).zip(0..n) {
            let block = Block {
                header: Header {
                    parent_hash,
                    // Hardcoded receipts_root matching the original test (same tx in each block)
                    receipts_root: b256!(
                        "0xd3a6acf9a244d78b33831df95d472c4128ea85bf079a1d41e32ed0b7d2244c9e"
                    ),
                    difficulty: chain_spec.fork(EthereumHardfork::Paris).ttd().expect("Paris TTD"),
                    number: i,
                    gas_limit: MIN_TRANSACTION_GAS,
                    gas_used: MIN_TRANSACTION_GAS,
                    ..Default::default()
                },
                body: BlockBody {
                    transactions: vec![sign_tx_with_key_pair(
                        key_pair,
                        Transaction::Eip2930(TxEip2930 {
                            chain_id: chain_spec.chain.id(),
                            nonce,
                            gas_limit: MIN_TRANSACTION_GAS,
                            gas_price: 1_500_000_000,
                            to: TxKind::Call(Address::ZERO),
                            value: U256::from(0.1 * ETH_TO_WEI as f64),
                            ..Default::default()
                        }),
                    )],
                    ..Default::default()
                },
            }
            .try_into_recovered()?;

            parent_hash = block.hash();
            blocks.push(block);
        }

        Ok(blocks)
    }

    fn execute_and_commit_blocks<N>(
        provider_factory: &ProviderFactory<N>,
        chain_spec: &Arc<ChainSpec>,
        blocks: &[RecoveredBlock<reth_ethereum_primitives::Block>],
    ) -> Result<()>
    where
        N: ProviderNodeTypes<
            Primitives: NodePrimitives<
                Block = reth_ethereum_primitives::Block,
                BlockBody = reth_ethereum_primitives::BlockBody,
                Receipt = reth_ethereum_primitives::Receipt,
            >,
        >,
    {
        for block in blocks {
            execute_block_and_commit_to_database(provider_factory, chain_spec.clone(), block)?;
        }
        Ok(())
    }

    #[tokio::test]
    async fn test_batch_parallel_range_advance() -> Result<()> {
        reth_tracing::init_test_tracing();

        // Create a key pair for the sender
        let key_pair = generators::generate_key(&mut generators::rng());
        let address =
            Address::from_raw_public_key(&key_pair.public_key().serialize_uncompressed()[1..]);

        let chain_spec = chain_spec(address);

        let executor = EthEvmConfig::ethereum(chain_spec.clone());
        let provider_factory = create_test_provider_factory_with_chain_spec(chain_spec.clone());
        init_genesis(&provider_factory)?;
        let blockchain_db = BlockchainProvider::new(provider_factory.clone())?;

        // Create and commit 4 blocks
        let blocks = create_blocks(&chain_spec, key_pair, 4)?;
        execute_and_commit_blocks(&provider_factory, &chain_spec, &blocks)?;

        // Create factory with batch size 2 (via thresholds max_blocks=2) and parallelism=2
        let factory = BackfillJobFactory::new(executor.clone(), blockchain_db.clone())
            .with_thresholds(ExecutionStageThresholds { max_blocks: Some(2), ..Default::default() })
            .with_stream_parallelism(2);

        // Stream backfill for range 1..=4
        let mut backfill_stream = factory.backfill(1..=4).into_stream();

        // Collect the two expected chains from the stream
        let chain1 = backfill_stream.next().await.unwrap()?;
        let chain2 = backfill_stream.next().await.unwrap()?;
        assert!(backfill_stream.next().await.is_none());

        // Compute expected chains using non-stream BackfillJob (sequential)
        let factory_seq =
            BackfillJobFactory::new(executor.clone(), blockchain_db.clone()).with_thresholds(
                ExecutionStageThresholds { max_blocks: Some(2), ..Default::default() },
            );

        let expected_chain1 =
            factory_seq.backfill(1..=2).collect::<Result<Vec<_>, _>>()?.into_iter().next().unwrap();
        let expected_chain2 =
            factory_seq.backfill(3..=4).collect::<Result<Vec<_>, _>>()?.into_iter().next().unwrap();

        // Assert the streamed chains match the expected sequential ones
        assert_eq!(chain1.blocks(), expected_chain1.blocks());
        assert_eq!(chain1.execution_outcome(), expected_chain1.execution_outcome());
        assert_eq!(chain2.blocks(), expected_chain2.blocks());
        assert_eq!(chain2.execution_outcome(), expected_chain2.execution_outcome());

        Ok(())
    }
}
