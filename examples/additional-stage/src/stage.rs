//! Example custom stage implementation.

use async_trait::async_trait;
use reth::providers::{BlockReader, DatabaseProviderRW};
use reth_db::{database::Database, transaction::DbTxMut};
use reth_primitives::stage::StageId;
use reth_stages::{
    ExecInput, ExecOutput, Stage, StageError, StageSet, StageSetBuilder, UnwindInput, UnwindOutput,
};

/// A single stage.
#[derive(Debug, Default)]
pub struct MyStage {}

impl MyStage {
    pub fn new() -> Self {
        MyStage {}
    }
}

#[async_trait]
impl<DB: Database> Stage<DB> for MyStage {
    fn id(&self) -> StageId {
        StageId::Other("MyStage")
    }

    async fn execute(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: ExecInput,
    ) -> Result<ExecOutput, StageError> {
        // For demonstration, the stage stores the most recent block that a "miner" produces.
        if let Some(block) = provider.block_by_number(input.next_block()?)? {
            provider.tx_mut().put(block.beneficiary, block.number)?;
        }
        todo!("Return ExecOutput");
    }

    async fn unwind(
        &mut self,
        provider: &DatabaseProviderRW<'_, &DB>,
        input: UnwindInput,
    ) -> Result<UnwindOutput, StageError> {
        todo!("Remove entries from database as appropriate for the unwind.")
    }
}

/// A group of stages that are related.
pub struct MyStageSet {}

impl<DB: Database> StageSet<DB> for MyStageSet {
    fn builder(self) -> StageSetBuilder<DB> {
        StageSetBuilder::default().add_stage(MyStage::default())
    }
}

impl MyStageSet {
    pub fn new() -> Self {
        todo!()
    }
}
