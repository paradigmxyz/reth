//! Customization of transaction prewarming and ordered engine execution.

use alloy_evm::block::ExecutableTxParts;
use reth_evm::{
    block::{BlockExecutionError, BlockExecutor},
    execute::ExecutableTxFor,
    BlockExecutorForEvm, ConfigureEvm, Database, Evm, EvmEnvFor, EvmErrorFor, EvmFor,
    HaltReasonFor, TxEnvFor,
};
use reth_primitives_traits::TxTy;
use reth_provider::EvmStateProviderBox;
use reth_revm::database::StateProviderDatabase;
use revm::context::result::ResultAndState;

/// Database used by engine transaction prewarming.
pub type PrewarmDatabase = StateProviderDatabase<EvmStateProviderBox>;

/// Customizes speculative prewarming and authoritative execution of a block.
///
/// The engine creates a fresh strategy for each block and shares clones with its prewarm workers.
/// Implementations can publish speculative results by transaction index and consume them in block
/// order during execution. Speculation must not commit state. Authoritative execution must use the
/// supplied executor so receipts, state hooks and trie updates follow the normal engine path.
/// Execution errors propagate without retry. Return a validation error to reject the block;
/// internal/provider failures remain engine processing errors.
pub trait PayloadExecutionStrategy<Evm: ConfigureEvm>: Clone + Send + Sync + 'static {
    /// Creates isolated execution state for one block. Do not share speculative results across
    /// blocks, including competing blocks at the same height.
    fn for_block(&self, _transaction_count: usize) -> Self {
        self.clone()
    }

    /// Whether execution consumes prewarm results and therefore needs prewarming even for small
    /// blocks or when ordinary cache prewarming is disabled.
    fn requires_prewarming(&self) -> bool {
        false
    }

    /// Whether the engine may use native BAL execution instead of these transaction hooks.
    /// Custom strategies use the ordered executor unless they explicitly permit bypassing it.
    fn allow_parallel_bal_execution(&self) -> bool {
        false
    }

    /// Configures the environment used for speculative execution.
    fn configure_prewarm_env(&self, env: &mut EvmEnvFor<Evm>) {
        env.cfg_env.disable_nonce_check = true;
        env.cfg_env.disable_balance_check = true;
    }

    /// Configures a newly created worker EVM, for example to enable action recording.
    fn configure_prewarm_evm(
        &self,
        evm: EvmFor<Evm, PrewarmDatabase>,
    ) -> EvmFor<Evm, PrewarmDatabase> {
        evm
    }

    /// Executes one speculative transaction. The returned state feeds the existing trie hint
    /// channel. Implementations may retain additional transaction-local execution data.
    fn prewarm_transaction<Tx: ExecutableTxParts<TxEnvFor<Evm>, TxTy<Evm::Primitives>>>(
        &self,
        _index: usize,
        evm: &mut EvmFor<Evm, PrewarmDatabase>,
        tx: Tx,
    ) -> Result<
        ResultAndState<HaltReasonFor<Evm>>,
        EvmErrorFor<Evm, <PrewarmDatabase as revm::Database>::Error>,
    > {
        let (env, _) = tx.into_parts();
        evm.transact(env)
    }

    /// Called when a dispatched prewarm transaction completes, including provider initialization
    /// failures and cancellation. A consumer waiting for speculative data must be released here.
    fn on_prewarm_finished(&self, _index: usize) {}

    /// Executes or replays one transaction in block order through the normal block executor.
    fn execute_transaction<'a, DB: Database + 'a, Tx: ExecutableTxFor<Evm>>(
        &self,
        _index: usize,
        executor: &mut BlockExecutorForEvm<'a, Evm, DB>,
        tx: Tx,
    ) -> Result<(), BlockExecutionError> {
        executor.execute_transaction(tx)?;
        Ok(())
    }
}

/// Ordinary cache prewarming followed by sequential EVM execution.
#[derive(Clone, Copy, Debug, Default)]
pub struct SequentialExecution;

impl<Evm: ConfigureEvm> PayloadExecutionStrategy<Evm> for SequentialExecution {
    fn allow_parallel_bal_execution(&self) -> bool {
        true
    }
}
