//! Executor Factory

use crate::{
    bundle_state::BundleStateWithReceipts, BlockReader, StateProvider, TransactionVariant,
};
use reth_interfaces::{executor::BlockExecutionError, provider::ProviderError};
use reth_primitives::{Address, Block, BlockNumber, ChainSpec, PruneModes, U256};
use std::{fmt::Debug, ops::RangeInclusive, time::Duration};
use tracing::debug;

/// Executor factory that would create the EVM with particular state provider.
///
/// It can be used to mock executor.
pub trait ExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a>;

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec;
}

/// Executor factory that would create the range block executor with a state provider.
pub trait RangeExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_provider_and_state<'a, Provider, SP>(
        &'a self,
        provider: Provider,
        sp: SP,
    ) -> Box<dyn PrunableBlockRangeExecutor + 'a>
    where
        Provider: BlockReader + 'a,
        SP: StateProvider + 'a;

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec;
}

impl<T: ExecutorFactory> RangeExecutorFactory for T {
    fn with_provider_and_state<'a, Provider, SP>(
        &'a self,
        provider: Provider,
        sp: SP,
    ) -> Box<dyn PrunableBlockRangeExecutor + 'a>
    where
        Provider: BlockReader + 'a,
        SP: StateProvider + 'a,
    {
        let executor = self.with_state(sp);
        Box::new(BlockRangeExecutorWrapper { provider, executor })
            as Box<dyn PrunableBlockRangeExecutor>
    }

    fn chain_spec(&self) -> &ChainSpec {
        <T as ExecutorFactory>::chain_spec(self)
    }
}

/// An executor capable of executing a block.
#[auto_impl::auto_impl(Box)]
pub trait BlockExecutor {
    /// Execute a block.
    ///
    /// The number of `senders` should be equal to the number of transactions in the block.
    ///
    /// If no senders are specified, the `execute` function MUST recover the senders for the
    /// provided block's transactions internally. We use this to allow for calculating senders in
    /// parallel in e.g. staged sync, so that execution can happen without paying for sender
    /// recovery costs.
    fn execute(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError>;

    /// Executes the block and checks receipts.
    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError>;

    /// Return bundle state. This is output of executed blocks.
    fn take_output_state(&mut self) -> BundleStateWithReceipts;

    /// Internal statistics of execution.
    fn stats(&self) -> BlockExecutorStats;

    /// Returns the size hint of current in-memory changes.
    fn size_hint(&self) -> Option<usize>;
}

/// A [BlockExecutor] capable of in-memory pruning of the data that will be written to the database.
#[auto_impl::auto_impl(Box)]
pub trait PrunableBlockExecutor: BlockExecutor {
    /// Set tip - highest known block number.
    fn set_tip(&mut self, tip: BlockNumber);

    /// Set prune modes.
    fn set_prune_modes(&mut self, prune_modes: PruneModes);
}

/// A block executor with range execution implementations.
pub trait BlockRangeExecutor {
    /// Execute the range of blocks.
    fn execute_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
        should_verify_receipts: bool,
    ) -> Result<(), BlockExecutionError>;

    /// Return bundle state. This is output of executed blocks.
    fn take_output_state(&mut self) -> BundleStateWithReceipts;

    /// Internal statistics of execution.
    fn stats(&self) -> BlockExecutorStats;

    /// Returns the size hint of current in-memory changes.
    fn size_hint(&self) -> Option<usize>;
}

/// Wrapper for [BlockExecutor] that implements [BlockRangeExecutor]
#[derive(Debug)]
pub struct BlockRangeExecutorWrapper<Provider, Executor> {
    provider: Provider,
    executor: Executor,
}

impl<Provider, Executor> BlockRangeExecutor for BlockRangeExecutorWrapper<Provider, Executor>
where
    Provider: BlockReader,
    Executor: BlockExecutor,
{
    /// Execute the range of blocks.
    fn execute_range(
        &mut self,
        range: RangeInclusive<BlockNumber>,
        should_verify_receipts: bool,
    ) -> Result<(), BlockExecutionError> {
        for block_number in range {
            let td = self
                .provider
                .header_td_by_number(block_number)
                .unwrap() // TODO:
                .ok_or_else(|| ProviderError::HeaderNotFound(block_number.into()))?;

            // we need the block's transactions but we don't need the transaction hashes
            let block = self
                .provider
                .block_with_senders(block_number, TransactionVariant::NoHash)
                .unwrap() // TODO:
                .ok_or_else(|| ProviderError::BlockNotFound(block_number.into()))?;

            let (block, senders) = block.into_components();
            if should_verify_receipts {
                self.executor.execute_and_verify_receipt(&block, td, Some(senders))?;
            } else {
                self.executor.execute(&block, td, Some(senders))?;
            }
        }

        Ok(())
    }

    fn take_output_state(&mut self) -> BundleStateWithReceipts {
        self.executor.take_output_state()
    }

    fn stats(&self) -> BlockExecutorStats {
        self.executor.stats()
    }

    fn size_hint(&self) -> Option<usize> {
        self.executor.size_hint()
    }
}

/// An [AsyncBlockExecutor] capable of in-memory pruning of the data that will be written to the
/// database.
pub trait PrunableBlockRangeExecutor: BlockRangeExecutor {
    /// Set tip - highest known block number.
    fn set_tip(&mut self, tip: BlockNumber);

    /// Set prune modes.
    fn set_prune_modes(&mut self, prune_modes: PruneModes);
}

impl<Provider, Executor> PrunableBlockRangeExecutor
    for BlockRangeExecutorWrapper<Provider, Executor>
where
    Provider: BlockReader,
    Executor: PrunableBlockExecutor,
{
    fn set_tip(&mut self, tip: BlockNumber) {
        self.executor.set_tip(tip)
    }

    fn set_prune_modes(&mut self, prune_modes: PruneModes) {
        self.executor.set_prune_modes(prune_modes)
    }
}

/// Block execution statistics. Contains duration of each step of block execution.
#[derive(Clone, Debug, Default)]
pub struct BlockExecutorStats {
    /// Execution duration.
    pub execution_duration: Duration,
    /// Time needed to apply output of revm execution to revm cached state.
    pub apply_state_duration: Duration,
    /// Time needed to apply post execution state changes.
    pub apply_post_execution_state_changes_duration: Duration,
    /// Time needed to merge transitions and create reverts.
    /// It this time transitions are applies to revm bundle state.
    pub merge_transitions_duration: Duration,
    /// Time needed to caclulate receipt roots.
    pub receipt_root_duration: Duration,
    /// Time needed to recovere senders.
    pub sender_recovery_duration: Duration,
}

impl BlockExecutorStats {
    /// Log duration to info level log.
    pub fn log_info(&self) {
        debug!(
            target: "evm",
            evm_transact = ?self.execution_duration,
            apply_state = ?self.apply_state_duration,
            apply_post_state = ?self.apply_post_execution_state_changes_duration,
            merge_transitions = ?self.merge_transitions_duration,
            receipt_root = ?self.receipt_root_duration,
            sender_recovery = ?self.sender_recovery_duration,
            "Execution time"
        );
    }
}
