use crate::StreamBackfillJob;
use std::{
    ops::RangeInclusive,
    time::{Duration, Instant},
};

use alloy_primitives::BlockNumber;
use reth_evm::execute::{
    BatchExecutor, BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider, Executor,
};
use reth_primitives::{Block, BlockWithSenders, Receipt};
use reth_primitives_traits::format_gas_throughput;
use reth_provider::{
    BlockReader, Chain, HeaderProvider, ProviderError, StateProviderFactory, TransactionVariant,
};
use reth_prune_types::PruneModes;
use reth_revm::database::StateProviderDatabase;
use reth_stages_api::ExecutionStageThresholds;
use reth_tracing::tracing::{debug, trace};

/// Backfill job started for a specific range.
///
/// It implements [`Iterator`] that executes blocks in batches according to the provided thresholds
/// and yields [`Chain`]
#[derive(Debug)]
pub struct BackfillJob<E, P> {
    pub(crate) executor: E,
    pub(crate) provider: P,
    pub(crate) prune_modes: PruneModes,
    pub(crate) thresholds: ExecutionStageThresholds,
    pub(crate) range: RangeInclusive<BlockNumber>,
    pub(crate) stream_parallelism: usize,
}

impl<E, P> Iterator for BackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    type Item = Result<Chain, BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.range.is_empty() {
            return None
        }

        Some(self.execute_range())
    }
}

impl<E, P> BackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: BlockReader + HeaderProvider + StateProviderFactory,
{
    /// Converts the backfill job into a single block backfill job.
    pub fn into_single_blocks(self) -> SingleBlockBackfillJob<E, P> {
        self.into()
    }

    /// Converts the backfill job into a stream.
    pub fn into_stream(self) -> StreamBackfillJob<E, P, Chain> {
        self.into()
    }

    fn execute_range(&mut self) -> Result<Chain, BlockExecutionError> {
        debug!(
            target: "exex::backfill",
            range = ?self.range,
            "Executing block range"
        );

        let mut executor = self.executor.batch_executor(StateProviderDatabase::new(
            self.provider.history_by_block_number(self.range.start().saturating_sub(1))?,
        ));
        executor.set_prune_modes(self.prune_modes.clone());

        let mut fetch_block_duration = Duration::default();
        let mut execution_duration = Duration::default();
        let mut cumulative_gas = 0;
        let batch_start = Instant::now();

        let mut blocks = Vec::new();
        for block_number in self.range.clone() {
            // Fetch the block
            let fetch_block_start = Instant::now();

            let td = self
                .provider
                .header_td_by_number(block_number)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            // we need the block's transactions along with their hashes
            let block = self
                .provider
                .sealed_block_with_senders(block_number.into(), TransactionVariant::WithHash)?
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            fetch_block_duration += fetch_block_start.elapsed();

            cumulative_gas += block.gas_used;

            // Configure the executor to use the current state.
            trace!(target: "exex::backfill", number = block_number, txs = block.body.transactions.len(), "Executing block");

            // Execute the block
            let execute_start = Instant::now();

            // Unseal the block for execution
            let (block, senders) = block.into_components();
            let (unsealed_header, hash) = block.header.split();
            let block =
                Block { header: unsealed_header, body: block.body }.with_senders_unchecked(senders);

            executor.execute_and_verify_one((&block, td).into())?;
            execution_duration += execute_start.elapsed();

            // TODO(alexey): report gas metrics using `block.header.gas_used`

            // Seal the block back and save it
            blocks.push(block.seal(hash));

            // Check if we should commit now
            let bundle_size_hint = executor.size_hint().unwrap_or_default() as u64;
            if self.thresholds.is_end_of_batch(
                block_number - *self.range.start(),
                bundle_size_hint,
                cumulative_gas,
                batch_start.elapsed(),
            ) {
                break
            }
        }

        let last_block_number = blocks.last().expect("blocks should not be empty").number;
        debug!(
            target: "exex::backfill",
            range = ?*self.range.start()..=last_block_number,
            block_fetch = ?fetch_block_duration,
            execution = ?execution_duration,
            throughput = format_gas_throughput(cumulative_gas, execution_duration),
            "Finished executing block range"
        );
        self.range = last_block_number + 1..=*self.range.end();

        let chain = Chain::new(blocks, executor.finalize(), None);
        Ok(chain)
    }
}

/// Single block Backfill job started for a specific range.
///
/// It implements [`Iterator`] which executes a block each time the
/// iterator is advanced and yields ([`BlockWithSenders`], [`BlockExecutionOutput`])
#[derive(Debug, Clone)]
pub struct SingleBlockBackfillJob<E, P> {
    pub(crate) executor: E,
    pub(crate) provider: P,
    pub(crate) range: RangeInclusive<BlockNumber>,
    pub(crate) stream_parallelism: usize,
}

impl<E, P> Iterator for SingleBlockBackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    type Item = Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError>;

    fn next(&mut self) -> Option<Self::Item> {
        self.range.next().map(|block_number| self.execute_block(block_number))
    }
}

impl<E, P> SingleBlockBackfillJob<E, P>
where
    E: BlockExecutorProvider,
    P: HeaderProvider + BlockReader + StateProviderFactory,
{
    /// Converts the single block backfill job into a stream.
    pub fn into_stream(
        self,
    ) -> StreamBackfillJob<E, P, (BlockWithSenders, BlockExecutionOutput<Receipt>)> {
        self.into()
    }

    pub(crate) fn execute_block(
        &self,
        block_number: u64,
    ) -> Result<(BlockWithSenders, BlockExecutionOutput<Receipt>), BlockExecutionError> {
        let td = self
            .provider
            .header_td_by_number(block_number)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

        // Fetch the block with senders for execution.
        let block_with_senders = self
            .provider
            .block_with_senders(block_number.into(), TransactionVariant::WithHash)?
            .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

        // Configure the executor to use the previous block's state.
        let executor = self.executor.executor(StateProviderDatabase::new(
            self.provider.history_by_block_number(block_number.saturating_sub(1))?,
        ));

        trace!(target: "exex::backfill", number = block_number, txs = block_with_senders.block.body.transactions.len(), "Executing block");

        let block_execution_output = executor.execute((&block_with_senders, td).into())?;

        Ok((block_with_senders, block_execution_output))
    }
}

impl<E, P> From<BackfillJob<E, P>> for SingleBlockBackfillJob<E, P> {
    fn from(job: BackfillJob<E, P>) -> Self {
        Self {
            executor: job.executor,
            provider: job.provider,
            range: job.range,
            stream_parallelism: job.stream_parallelism,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{
        backfill::test_utils::{blocks_and_execution_outputs, chain_spec, to_execution_outcome},
        BackfillJobFactory,
    };
    use reth_blockchain_tree::noop::NoopBlockchainTree;
    use reth_db_common::init::init_genesis;
    use reth_evm_ethereum::execute::EthExecutorProvider;
    use reth_primitives::public_key_to_address;
    use reth_provider::{
        providers::BlockchainProvider, test_utils::create_test_provider_factory_with_chain_spec,
    };
    use reth_testing_utils::generators;
    use secp256k1::Keypair;

    #[test]
    fn test_backfill() -> eyre::Result<()> {
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

        let blocks_and_execution_outputs =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;
        let (block, block_execution_output) = blocks_and_execution_outputs.first().unwrap();
        let execution_outcome = to_execution_outcome(block.number, block_execution_output);

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor, blockchain_db);
        let job = factory.backfill(1..=1);
        let chains = job.collect::<Result<Vec<_>, _>>()?;

        // Assert that the backfill job produced the same chain as we got before when we were
        // executing only the first block
        assert_eq!(chains.len(), 1);
        let mut chain = chains.into_iter().next().unwrap();
        chain.execution_outcome_mut().bundle.reverts.sort();
        assert_eq!(chain.blocks(), &[(1, block.clone())].into());
        assert_eq!(chain.execution_outcome(), &execution_outcome);

        Ok(())
    }

    #[test]
    fn test_single_block_backfill() -> eyre::Result<()> {
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

        let blocks_and_execution_outcomes =
            blocks_and_execution_outputs(provider_factory, chain_spec, key_pair)?;

        // Backfill the first block
        let factory = BackfillJobFactory::new(executor, blockchain_db);
        let job = factory.backfill(1..=1);
        let single_job = job.into_single_blocks();
        let block_execution_it = single_job.into_iter();

        // Assert that the backfill job only produces a single block
        let blocks_and_outcomes = block_execution_it.collect::<Vec<_>>();
        assert_eq!(blocks_and_outcomes.len(), 1);

        // Assert that the backfill job single block iterator produces the expected output for each
        // block
        for (i, res) in blocks_and_outcomes.into_iter().enumerate() {
            let (block, mut execution_output) = res?;
            execution_output.state.reverts.sort();

            let sealed_block_with_senders = blocks_and_execution_outcomes[i].0.clone();
            let expected_block = sealed_block_with_senders.unseal();
            let expected_output = &blocks_and_execution_outcomes[i].1;

            assert_eq!(block, expected_block);
            assert_eq!(&execution_output, expected_output);
        }

        Ok(())
    }
}
