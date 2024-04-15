//! Ethereum executor.

use reth_evm::{
    execute::{
        BatchBlockOutput, BatchExecutor, EthBatchOutput, EthBlockExecutionInput, EthBlockOutput,
        Executor,
    },
    ConfigureEvm,
};
use reth_interfaces::executor::BlockExecutionError;
use reth_primitives::{Address, BlockWithSenders, ChainSpec, PruneModes, Receipts, U256};
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
    ) -> Result<(Receipts, u64), BlockExecutionError> {
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
    type Output = EthBlockOutput<Receipts>;
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

pub struct EthBatchExecutor<Evm, DB> {
    /// The
    executor: EthBlockExecutor<Evm, DB>,
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
    type Output = EthBatchOutput<Receipts>;
    type Error = BlockExecutionError;

    fn execute_one(&mut self, input: Self::Input<'_>) -> Result<BatchBlockOutput, Self::Error> {
        todo!()
    }

    fn finalize(self) -> Self::Output {
        todo!()
    }
}
