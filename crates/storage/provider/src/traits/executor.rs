//! Executor Factory

use crate::{bundle_state::BundleStateWithReceipts, StateProvider};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, BlockNumber, ChainSpec, PruneModes, U256};
use std::time::Duration;
use tracing::debug;

/// Executor factory that would create the EVM with particular state provider.
///
/// It can be used to mock executor.
pub trait ExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        _sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a>;

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec;
}

/// An executor capable of executing a block.
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
pub trait PrunableBlockExecutor: BlockExecutor {
    /// Set tip - highest known block number.
    fn set_tip(&mut self, tip: BlockNumber);

    /// Set prune modes.
    fn set_prune_modes(&mut self, prune_modes: PruneModes);
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
