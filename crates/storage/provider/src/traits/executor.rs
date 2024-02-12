//! Executor Factory

use crate::{bundle_state::BundleStateWithReceipts, StateProvider};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt, U256};
use std::time::Duration;
use tracing::debug;

/// re-export node-api BlockExecutor for now
pub use reth_node_api::evm::BlockExecutor;

/// Executor factory that would create the EVM with particular state provider.
///
/// It can be used to mock executor.
pub trait ExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        _sp: SP,
    ) -> Box<dyn PrunableBlockExecutor + 'a>;
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
    /// Time needed to calculate receipt roots.
    pub receipt_root_duration: Duration,
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
            "Execution time"
        );
    }
}
