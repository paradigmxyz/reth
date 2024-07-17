use reth_evm::execute::{
    BlockExecutionError, BlockExecutionOutput, BlockExecutorProvider, Executor,
};
use reth_primitives::{BlockNumber, BlockWithSenders, Receipt};
use reth_provider::{
    BlockReader, HeaderProvider, ProviderError, StateProviderFactory, TransactionVariant,
};
use reth_revm::database::StateProviderDatabase;
use reth_tracing::tracing::trace;
use std::ops::RangeInclusive;

use crate::BackfillJob;

/// Single block Backfill job started for a specific range.
///
/// It implements [`Iterator`] which executes a block each time the
/// iterator is advanced and yields ([`BlockWithSenders`], [`BlockExecutionOutput`])
#[derive(Debug, Clone)]
pub struct SingleBlockBackfillJob<E, P> {
    executor: E,
    provider: P,
    pub(crate) range: RangeInclusive<BlockNumber>,
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

        trace!(target: "exex::backfill", number = block_number, txs = block_with_senders.block.body.len(), "Executing block");

        let block_execution_output = executor.execute((&block_with_senders, td).into())?;

        Ok((block_with_senders, block_execution_output))
    }
}

impl<E, P> From<BackfillJob<E, P>> for SingleBlockBackfillJob<E, P> {
    fn from(value: BackfillJob<E, P>) -> Self {
        Self { executor: value.executor, provider: value.provider, range: value.range }
    }
}
