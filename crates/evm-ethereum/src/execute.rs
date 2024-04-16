//! Ethereum executor.

use std::sync::Arc;
use revm_primitives::db::{Database, DatabaseCommit};
use reth_evm::{
    ConfigureEvm,
    execute::{BatchBlockOutput, BatchExecutor, EthBlockExecutionInput, EthBlockOutput, Executor},
};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{
    BlockWithSenders, ChainSpec, Receipt, U256,
};
use reth_provider::BundleStateWithReceipts;
use reth_revm::{stack::InspectorStack, State};
use reth_revm::batch::BlockBatchRecord;

/// A basic Ethereum block executor.
///
/// Expected usage:
/// - Create a new instance of the executor.
/// - Execute the block.
#[derive(Debug)]
pub struct EthBlockExecutor<Evm, DB> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm_config: Evm,
    /// The state to use for execution
    state: State<DB>,
    /// Optional inspector stack for debugging
    inspector: Option<InspectorStack>,
}

impl<Evm, DB> EthBlockExecutor<Evm, DB> {
    /// Creates a new Ethereum block executor.
    pub fn new(chain_spec: Arc<ChainSpec>, evm: Evm, state: State<DB>) -> Self {
        Self { chain_spec, evm_config: evm, state, inspector: None }
    }

    /// Sets the inspector stack for debugging.
    pub fn with_inspector(mut self, inspector: InspectorStack) -> Self {
        self.inspector = Some(inspector);
        self
    }
}

impl<Evm, DB> EthBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + DatabaseCommit,
{
    /// Execute a single block and apply the state changes to the internal state.
    pub(crate) fn execute_block(
        &mut self,
        block: &BlockWithSenders,
        total_difficulty: U256,
    ) -> Result<(Vec<Receipt>, u64), BlockExecutionError> {
        todo!()
    }

    /// Invoked before transactions are executed
    fn pre_execution(&self) {
        todo!()
    }

    /// Apply post execution state changes, including block rewards, withdrawals, and irregular DAO
    /// hardfork state change.
    fn post_execution(&self) {
        todo!()
    }
}

impl<Evm, DB> Executor for EthBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + DatabaseCommit,
{
    type Input<'a> = EthBlockExecutionInput<'a, BlockWithSenders>;
    type Output = EthBlockOutput<Receipt>;
    type Error = BlockExecutionError;

    /// Executes the block and commits the state changes.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed or failed verification.
    ///
    /// State changes are committed to the database.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_block(block, total_difficulty)?;
        Ok(EthBlockOutput { state: self.state.take_bundle(), receipts, gas_used })
    }
}


/// An executor for a batch of blocks.
///
/// State changes are tracked until the executor is finalized.
#[derive(Debug)]
pub struct EthBatchExecutor<Evm, DB> {
    /// The executor used to execute blocks.
    executor: EthBlockExecutor<Evm, DB>,
    /// Keeps track of the batch and record receipts based on the configured prune mode
    batch_record: BlockBatchRecord,
    stats: (),
}

impl<Evm, DB> BatchExecutor for EthBatchExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + DatabaseCommit,
{
    type Input<'a> = EthBlockExecutionInput<'a, BlockWithSenders>;
    type Output = BundleStateWithReceipts;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, _gas_used) = self.executor.execute_block(block, total_difficulty)?;
        self.batch_record.save_receipts(receipts)?;

        Ok(BatchBlockOutput { size_hint: Some(self.executor.state.bundle_size_hint()) })
    }

    fn finalize(mut self) -> Self::Output {
        // TODO log stats
        BundleStateWithReceipts::new(
            self.executor.state.take_bundle(),
            self.batch_record.take_receipts(),
            self.batch_record.first_block().unwrap_or_default(),
        )
    }
}
