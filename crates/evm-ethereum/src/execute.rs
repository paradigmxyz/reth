//! Ethereum executor.
//!
//! TODO refactor this so it can be reused by other networks.

use reth_evm::{
    execute::{BatchBlockOutput, BatchExecutor, EthBlockExecutionInput, EthBlockOutput, Executor},
    ConfigureEvm,
};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{
    BlockNumber, BlockWithSenders, ChainSpec, PruneModes, Receipt, Receipts, U256,
};
use reth_provider::BundleStateWithReceipts;
use reth_revm::{stack::InspectorStack, State};
use revm_primitives::db::{Database, DatabaseCommit};
use std::sync::Arc;

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
    /// Returns an error if the block could not be executed.
    ///
    /// State changes are committed to the database.
    fn execute(mut self, input: Self::Input<'_>) -> Result<Self::Output, Self::Error> {
        let EthBlockExecutionInput { block, total_difficulty } = input;
        let (receipts, gas_used) = self.execute_block(block, total_difficulty)?;
        Ok(EthBlockOutput { state: self.state.take_bundle(), receipts, gas_used })
    }
}

/// TODO make this reusable for other networks.
pub struct EthBatchExecutor<Evm, DB> {
    /// The executor used to execute blocks.
    executor: EthBlockExecutor<Evm, DB>,
    /// The collection of receipts.
    /// Outer vector stores receipts for each block sequentially.
    /// The inner vector stores receipts ordered by transaction number.
    ///
    /// If receipt is None it means it is pruned.
    receipts: Receipts,
    /// First block will be initialized to `None`
    /// and be set to the block number of first block executed.
    first_block: Option<BlockNumber>,

    /// Pruning configuration.
    ///
    /// TODO: make everything related to pruning a separate type.
    prune_mode: PruneModes,

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

        let mut receipts = receipts.into_iter().map(Option::Some).collect();

        // TODO: prune receipts according to the prune mode

        self.receipts.push(receipts);

        Ok(BatchBlockOutput { size_hint: Some(self.executor.state.bundle_size_hint()) })
    }

    fn finalize(mut self) -> Self::Output {
        let receipts = std::mem::take(&mut self.receipts);
        BundleStateWithReceipts::new(
            self.executor.state.take_bundle(),
            receipts,
            self.first_block.unwrap_or_default(),
        )
    }
}
