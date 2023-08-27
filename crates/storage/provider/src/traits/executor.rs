//! Executor Factory

use std::time::Duration;

use crate::{change::BundleState, StateProvider};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, Block, ChainSpec, U256};
use tracing::info;

/// Executor factory that would create the EVM with particular state provider.
///
/// It can be used to mock executor.
pub trait ExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_sp<'a, SP: StateProvider + 'a>(&'a self, _sp: SP) -> Box<dyn BlockExecutor + 'a>;

    /// Return internal chainspec
    fn chain_spec(&self) -> &ChainSpec;
}

/// Statistic data of bock execution
#[derive(Clone, Debug, Default)]
pub struct BlockExecutorStats {
    /// How long did duration take
    pub execution_duration: Duration,
    /// How long did output of EVM execution take
    /// to apply to cache state
    pub apply_state_duration: Duration,
    /// Apply Post state execution changes duration.
    pub apply_post_execution_changes_duration: Duration,
    /// How long did it take to merge transitions and create
    /// reverts. This applies data to evm bundle state.
    pub merge_transitions_duration: Duration,
    /// How long did it take to calculate receipt root.
    pub receipt_root_duration: Duration,
    /// If no senders are send how long did it
    /// take to recover them
    pub sender_recovery_duration: Duration,
}

impl BlockExecutorStats {
    /// Log duration to info level log.
    pub fn log_info(&self) {
        info!(target: "evm",
            evm_transact = ?self.execution_duration,
            apply_state = ?self.apply_state_duration,
            apply_post_state = ?self.apply_post_execution_changes_duration,
            merge_transitions = ?self.merge_transitions_duration,
            receipt_root = ?self.receipt_root_duration,
            sender_recovery = ?self.sender_recovery_duration,
            "Execution time");
    }
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

    /// Executes the block and checks receipts
    fn execute_and_verify_receipt(
        &mut self,
        block: &Block,
        total_difficulty: U256,
        senders: Option<Vec<Address>>,
    ) -> Result<(), BlockExecutionError>;

    /// Internal statistics of execution.
    fn stats(&self) -> BlockExecutorStats;

    /// Return bundle state. This is output of the execution.
    fn take_output_state(&mut self) -> BundleState;
}
