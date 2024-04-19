//! Executor Factory

use crate::{bundle_state::BundleStateWithReceipts, StateProvider};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{BlockNumber, BlockWithSenders, PruneModes, Receipt, U256};

/// Executor factory that would create the EVM with particular state provider.
///
/// It can be used to mock executor.
pub trait ExecutorFactory: Send + Sync + 'static {
    /// Executor with [`StateProvider`]
    fn with_state<'a, SP: StateProvider + 'a>(
        &'a self,
        _sp: SP,
    ) -> Box<dyn PrunableBlockExecutor<Error = BlockExecutionError> + 'a>;
}

/// An executor capable of executing a block.
///
/// This type is capable of executing (multiple) blocks by applying the state changes made by each
/// block. The final state of the executor can extracted using
/// [take_output_state](BlockExecutor::take_output_state).
pub trait BlockExecutor {
    /// The error type returned by the executor.
    type Error;

    /// Executes the entire block and verifies:
    ///  - receipts (receipts root)
    ///
    /// This will update the state of the executor with the changes made by the block.
    fn execute_and_verify_receipt(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), Self::Error>;

    /// Runs the provided transactions and commits their state to the run-time database.
    ///
    /// The returned [BundleStateWithReceipts] can be used to persist the changes to disk, and
    /// contains the changes made by each transaction.
    ///
    /// The changes in [BundleStateWithReceipts] have a transition ID associated with them: there is
    /// one transition ID for each transaction (with the first executed tx having transition ID
    /// 0, and so on).
    ///
    /// The second returned value represents the total gas used by this block of transactions.
    ///
    /// See [execute_and_verify_receipt](BlockExecutor::execute_and_verify_receipt) for more
    /// details.
    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), Self::Error>;

    /// Return bundle state. This is output of executed blocks.
    fn take_output_state(&mut self) -> BundleStateWithReceipts;

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
