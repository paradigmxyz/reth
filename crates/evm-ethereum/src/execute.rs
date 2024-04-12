//! Ethereum executor.

use reth_evm::{
    execute::{EthBlockInput, EthBlockOutput, Executor},
    ConfigureEvm,
};
use reth_primitives::{BlockWithSenders, ChainSpec, Receipts};
use revm_primitives::db::{Database, DatabaseCommit};
use std::sync::Arc;
use reth_revm::stack::InspectorStack;

/// A basic Ethereum block executor.
#[derive(Debug)]
pub struct EthBlockExecutor<Evm, DB> {
    /// The chainspec
    chain_spec: Arc<ChainSpec>,
    /// How to create an EVM.
    evm: Evm,
    /// Optional inspector stack for debugging
    inspector: Option<InspectorStack>,
    /// The database to use for execution
    db: DB,
}

impl<Evm, DB> EthBlockExecutor<Evm, DB> {
    /// Creates a new Ethereum block executor.
    pub fn new(chain_spec: Arc<ChainSpec>, evm: Evm, db: DB) -> Self {
        Self { chain_spec, evm, inspector: None, db }
    }

    /// Sets the inspector stack for debugging.
    pub fn with_inspector(mut self, inspector: InspectorStack) -> Self {
        self.inspector = Some(inspector);
        self
    }
}

impl<Evm, DB> EthBlockExecutor<Evm, DB> where Evm: ConfigureEvm {}

impl<Evm, DB, Block> Executor for EthBlockExecutor<Evm, DB>
where
    Evm: ConfigureEvm,
    DB: Database + DatabaseCommit,
    Block: EthBlockInput<BlockWithSenders>,
{
    type Input = Block;
    type Output = EthBlockOutput<Receipts>;
    type Error = ();

    /// Executes the block and commits the state changes.
    ///
    /// Returns the receipts of the transactions in the block.
    ///
    /// Returns an error if the block could not be executed.
    ///
    /// State changes are committed to the database.
    fn execute(self, _block: Self::Input) -> Result<Self::Output, Self::Error> {

        todo!()
    }
}
