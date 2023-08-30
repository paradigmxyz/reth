//! Example custom stage implementation.

use async_trait::async_trait;
use reth::providers::{BlockReader, DatabaseProviderRW};
use reth_db::{database::Database, transaction::DbTxMut};
use reth_primitives::stage::{StageCheckpoint, StageId};
use reth_stages::{
    ExecInput, ExecOutput, Stage, StageError, StageSet, StageSetBuilder, UnwindInput, UnwindOutput,
};

use crate::table::MyTable;

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
        if input.target_reached() {
            return Ok(ExecOutput::done(input.checkpoint()))
        }
        // For demonstration, the stage stores the most recent block that a "miner" produces.
        let range = input.next_block_range();
        let Some(range_end) = range.last() else { return Ok(ExecOutput::done(input.checkpoint())) };
        for block_number in range {
            if let Some(block) = provider.block_by_number(block_number)? {
                provider.tx_mut().put::<MyTable>(block.beneficiary, block.number)?;
                provider.commit()?;
            }
        }
        Ok(ExecOutput { checkpoint: StageCheckpoint::new(range_end), done: input.target_reached() })
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
