use crate::evm::BundleStateWithReceipts;
use reth_primitives::{
    revm::env::fill_block_env, Address, BlockWithSenders, ChainSpec, Header, Receipt, Transaction,
    U256,
};
use revm::Database;
use revm_primitives::{BlockEnv, CfgEnvWithHandlerCfg, SpecId, TxEnv};

/// Trait for configuring the EVM for executing full blocks.
pub trait EvmConfig {
    /// The type that can executes transactions and full blocks.
    type Executor: BlockExecutor;

    /// A hook that allows to modify the EVM before execution.
    fn evm(&self, db: impl Database) -> Self::Executor;

    /// A hook that allows to modify the EVM before execution, with an inspector.
    fn evm_with_inspector<I>(&self, db: impl Database, inspector: I) -> Self::Executor;
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
    /// The error type returned by the executor.
    type Error;

    /// Execute a block.
    fn execute(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(), Self::Error>;

    /// Executes the block and checks receipts.
    ///
    /// See [execute](BlockExecutor::execute) for more details.
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
    /// See [execute](BlockExecutor::execute) for more details.
    fn execute_transactions(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), Self::Error>;

    /// Return bundle state. This is output of executed blocks.
    fn take_output_state(&mut self) -> BundleStateWithReceipts;
}
