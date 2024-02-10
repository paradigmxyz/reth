use reth_primitives::{revm::env::fill_block_env, Address, ChainSpec, Header, Transaction, U256, BlockWithSenders, Receipt};
use revm::Database;
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, SpecId, TxEnv};

/// Trait for configuring the EVM.
pub trait EvmConfig: ConfigureEvmEnv {
    // TODO: It would be great if this didn't need to be a GAT, this is only because EVMProcessor
    // has a lifetime parameter.
    // _that_ is only because Evm has a lifetime parameter.
    // _that_ is because Handler and DBBox have lifetime parameters.
    /// The type that can executes transactions and full blocks.
    type Executor<'a>: BlockExecutor where Self: 'a;

    /// A hook that allows to modify the EVM before execution.
    fn evm(&self, db: impl Database) -> Self::Executor<'_>;

    /// A hook that allows to modify the EVM before execution, with an inspector.
    fn evm_with_inspector<I>(&self, db: impl Database, inspector: I) -> Self::Executor<'_>;
}

/// This represents the set of methods used to configure the EVM before execution.
pub trait ConfigureEvmEnv: Send + Sync + Unpin + Clone {
    /// The type of the transaction metadata.
    type TxMeta;

    /// Fill transaction environment from a [Transaction] and the given sender address.
    fn fill_tx_env<T>(tx_env: &mut TxEnv, transaction: T, sender: Address, meta: Self::TxMeta)
    where
        T: AsRef<Transaction>;

    /// Fill [CfgEnvWithHandlerCfg] fields according to the chain spec and given header
    fn fill_cfg_env(
        cfg_env: &mut CfgEnvWithHandlerCfg,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    );

    /// Convenience function to call both [fill_cfg_env](ConfigureEvmEnv::fill_cfg_env) and
    /// [fill_block_env].
    fn fill_cfg_and_block_env(
        cfg: &mut CfgEnvWithHandlerCfg,
        block_env: &mut BlockEnv,
        chain_spec: &ChainSpec,
        header: &Header,
        total_difficulty: U256,
    ) {
        Self::fill_cfg_env(cfg, chain_spec, header, total_difficulty);
        let after_merge = cfg.handler_cfg.spec_id >= SpecId::MERGE;
        fill_block_env(block_env, chain_spec, header, after_merge);
    }
}

/// An executor capable of executing a block.
pub trait BlockExecutor {
    /// Execute a block.
    fn execute(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError>;

    /// Executes the block and checks receipts.
    ///
    /// See [execute](BlockExecutor::execute) for more details.
    fn execute_and_verify_receipt(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), BlockExecutionError>;

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
    /// See [execute](BlockExecutor::execute) for more details.
    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError>;

    /// Return bundle state. This is output of executed blocks.
    fn take_output_state(&mut self) -> BundleStateWithReceipts;

    /// Internal statistics of execution.
    fn stats(&self) -> BlockExecutorStats;

    /// Returns the size hint of current in-memory changes.
    fn size_hint(&self) -> Option<usize>;
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
